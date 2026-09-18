local M = {}

local Config = {
    enabled = true,
    retries = 3,
}

local function greet(name)
    if not Config.enabled then
        return "disabled"
    end
    return string.format("Hello, %s!", name)
end

for _, name in ipairs({ "Zcv", "Lua" }) do
    print(greet(name), Config.retries)
end

return M
