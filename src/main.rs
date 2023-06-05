use std::collections::HashMap;
use std::fs;
use std::path::PathBuf;

use exif::Field;
use exif::In;
use exif::Tag;
use exif::Value;

extern crate exif;

#[derive(serde::Deserialize)]
struct AddressComponent {
    long_name: Option<String>,
    short_name: Option<String>,
    types: Vec<String>,
}
#[derive(serde::Deserialize)]
struct GoogleApiResponseResult {
    address_components: Vec<AddressComponent>,
}
#[derive(serde::Deserialize)]
struct GoogleApiResponse {
    results: Vec<GoogleApiResponseResult>,
}

#[derive(serde::Deserialize, serde::Serialize, Default)]
struct Cache {
    latlon: HashMap<String, String>,
}

impl Cache {
    fn config_path() -> PathBuf {
        let mut env = std::env::current_exe().unwrap();
        env.pop();
        env.push("cache.json");
        return env;
    }
    fn restore() -> Self {
        match std::fs::read_to_string(Cache::config_path()) {
            Ok(json) => match serde_json::from_str::<Self>(&json) {
                Ok(cache) => cache,
                Err(_) => Cache::default(),
            },
            Err(_) => Cache::default(),
        }
    }

    fn save(&self) {
        std::fs::write(
            Cache::config_path(),
            serde_json::to_string_pretty(self).expect("Failed to serialize cache"),
        )
        .expect("Failed to save cache");
    }
}

#[tokio::main]
async fn main() -> tokio::io::Result<()> {
    let args: Vec<String> = std::env::args().collect();
    if args.len() < 3 {
        panic!("Brakuje parametrów.")
    }

    let paths = fs::read_dir(args.get(1).unwrap()).unwrap();
    let api_key = String::from(args.get(2).unwrap());
    let make_changes = args.get(3).map(|o| o == "wykonaj").unwrap_or(false);

    let client = reqwest::Client::new();

    let mut cache = Cache::restore();
    let mut rename_plan: HashMap<PathBuf, String> = HashMap::new();

    for (index, path) in paths.enumerate() {
        let latlon_cache = &mut cache.latlon;
        let path = path.unwrap().path();
        let file = std::fs::File::open(&path).unwrap();
        let mut bufreader = std::io::BufReader::new(&file);
        let exifreader = exif::Reader::new();
        let exif = exifreader.read_from_container(&mut bufreader);

        if let Ok(exif) = exif {
            let city_name = city_name(&client, &exif, latlon_cache, &api_key).await;
            let creation_date = creation_date(&exif);

            let new_file_name = format!("{:04}{}{}", index + 1, creation_date, city_name);
            rename_plan.insert(path.clone(), new_file_name);
        } else {
            rename_plan.insert(path.clone(), format!("{:04}", index + 1));
        }
    }
    cache.save();

    rename_plan.iter().for_each(|entry| {
        let mut new_path = entry.0.clone();

        let extension = {
            let path_e = entry.0.clone();
            let path = path_e.as_path();
            let path = path.extension().unwrap();
            path.to_str().unwrap().to_owned()
        };

        new_path.pop();
        new_path.push(format!("{}.{}", entry.1, extension));
        println!("{} -> {}", entry.0.display(), new_path.display());
        if make_changes {
            fs::rename(entry.0, new_path).unwrap();
        }
    });

    return Ok(());
}

async fn city_name(
    client: &reqwest::Client,
    exif: &exif::Exif,
    latlon_cache: &mut HashMap<String, String>,
    api_key: &str,
) -> String {
    let lat = rational_value(exif.get_field(Tag::GPSLatitude, In::PRIMARY));
    let lon = rational_value(exif.get_field(Tag::GPSLongitude, In::PRIMARY));
    if let Ok(lat) = lat {
        if let Ok(lon) = lon {
            let latlon = format!("{:.6},{:.6}", &lat, &lon);
            if let Some(city_name) = latlon_cache.get(&latlon) {
                return format!("_{}", city_name).to_owned();
            } else {
                let query = format!("https://maps.googleapis.com/maps/api/geocode/json?latlng={}&language=pl&result_type=administrative_area_level_3&key={}", &latlon, &api_key);

                let response = client.get(&query).send().await.unwrap();
                let json_text = response.text().await.unwrap();
                let city_name = find_ciy_name(&json_text);
                return match &city_name {
                    Ok(city_name) => {
                        latlon_cache.insert(latlon.clone(), city_name.clone());
                        format!("_{}", city_name).to_owned()
                    }
                    Err(city_name) => {
                        latlon_cache.insert(latlon.clone(), city_name.clone());
                        String::from("")
                    }
                };
            }
        }
    }
    return String::from("");
}

fn find_ciy_name(json_text: &str) -> Result<String, String> {
    let political = String::from("political");
    let city = String::from("administrative_area_level_3");
    if let Ok(json) = serde_json::from_str::<'_, GoogleApiResponse>(&json_text) {
        for result in json.results {
            for component in result.address_components {
                let types = component.types;
                if types.contains(&political) && types.contains(&city) {
                    if let Some(long_name) = component.long_name {
                        return Ok(long_name);
                    }
                    if let Some(short_name) = component.short_name {
                        return Ok(short_name);
                    }
                    return Err(String::from("NIEZNANE"));
                }
            }
        }
    }

    return Err(String::from("NIEZNALEZIONE"));
}
fn creation_date(exif: &exif::Exif) -> String {
    let mut dates: Vec<String> = vec![];
    get_tag_date(exif, Tag::DateTime, &mut dates);
    get_tag_date(exif, Tag::DateTimeDigitized, &mut dates);
    get_tag_date(exif, Tag::DateTimeOriginal, &mut dates);
    dates.sort();
    return dates.first().map(|d| format!("_{}", d)).unwrap_or(String::from(""));
}
fn get_tag_date(exif: &exif::Exif, tag: Tag, dates: &mut Vec<String>) {
    let date = exif.get_field(tag, In::PRIMARY);
    if let Some(date) = date {
        return match date.value {
            Value::Ascii(ref v) if !v.is_empty() => {
                let found_date = date.display_value().to_string();
                dates.push(found_date.replace(":", "."))
            }
            _ => {}
        };
    }
}

fn rational_value(field: Option<&Field>) -> Result<f64, String> {
    match field {
        Some(field) => {
            return match field.value {
                Value::Rational(ref v) if !v.is_empty() => {
                    let h = v[0].to_f64();
                    let min = v[1].to_f64() / 60.0;
                    let s = v[2].to_f64() / 60.0 / 60.0;
                    return Ok(h + min + s);
                }
                _ => Err(String::from("not a rational value")),
            }
        }
        None => Err(String::from("missing field")),
    }
}
