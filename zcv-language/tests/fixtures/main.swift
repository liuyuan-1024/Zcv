import Foundation

struct Config {
    let name: String
    var enabled = true
    var retries = 3
}

func greeting(for config: Config) -> String {
    guard config.enabled else { return "disabled" }
    return "Hello, \(config.name)!"
}

let configs = [Config(name: "Zcv"), Config(name: "Swift", retries: 1)]
for config in configs {
    print(greeting(for: config), config.retries)
}
