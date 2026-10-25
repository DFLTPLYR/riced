-- Copy this project's apps/ directory beside your installed config.lua,
-- then add the apps entry below to your configuration.
return {
    apps = {
        { name = "bar", props = { height = 32 } },
    },
    -- Legacy bars remain supported; do not enable both on the same edge.
    bar = {},
}
