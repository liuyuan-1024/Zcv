package main

import "fmt"

type Config struct {
	Name    string
	Enabled bool
}

func greeting(config Config) string {
	if !config.Enabled {
		return "disabled"
	}
	return fmt.Sprintf("Hello, %s!", config.Name)
}

func main() {
	config := Config{Name: "Zcv", Enabled: true}
	fmt.Println(greeting(config))
}
