from dataclasses import dataclass


@dataclass
class Config:
    name: str
    enabled: bool = True
    retries: int = 3


def greet(config: Config) -> str:
    if not config.enabled:
        return "disabled"
    return f"Hi {config.name}!"


for config in [Config("Zcv"), Config("Python", retries=1)]:
    print(greet(config))
