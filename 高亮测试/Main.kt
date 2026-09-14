package sample

data class Config(
    val name: String,
    val enabled: Boolean = true,
    val retries: Int = 3,
)

fun greet(config: Config): String {
    if (!config.enabled) return "disabled"
    return "Hello, ${config.name}!"
}

fun main() {
    val configs = listOf(Config("Zcv"), Config("Kotlin", retries = 1))
    configs.forEach { println(greet(it)) }
}
