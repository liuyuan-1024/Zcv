use std::collections::HashMap;

#[derive(Debug)]
struct Config {
    enabled: bool,
    retries: u32,
}

fn greeting(name: &str) -> String {
    format!("Hello, {name}!")
}

fn main() {
    let config = Config {
        enabled: true,
        retries: 3,
    };
    let mut values = HashMap::new();
    values.insert("message", greeting("Zcv"));
    if config.enabled {
        println!("{} ({})", values["message"], config.retries);
    }
}
