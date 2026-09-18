<?php

declare(strict_types=1);

final class Config
{
    public function __construct(
        public readonly string $name,
        public readonly bool $enabled = true,
    ) {}
}

function greet(Config $config): string
{
    return $config->enabled ? "Hello, {$config->name}!" : "disabled";
}

foreach ([new Config("Zcv"), new Config("PHP")] as $config) {
    echo greet($config), PHP_EOL;
}
