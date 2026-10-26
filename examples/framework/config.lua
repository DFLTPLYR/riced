-- Copy this project's apps/ and extensions/ directories beside your
-- config.lua, then add the entries below to your configuration.
return {
    extensions = {
        -- The in-repo example points at the shared palette crate. A copied
        -- project uses the default path: extensions/palette.
        { name = "palette", path = "../../extensions/palette" },
    },
    apps = {
        { name = "bar", props = { height = 32 } },
        { name = "palette" },
    },
    -- Legacy bars remain supported; do not enable both on the same edge.
    bar = {},
}
