-- One bar. The file stem is the bar name; root.lua lists data/bars and
-- declares one layer per file. Settings → Panel reads and writes this file.
return {
    edge = "top",
    thickness = 34,
    slots = {
        { align = "start", widgets = { "workspaces" } },
        { align = "center", widgets = { "clock" } },
        { align = "end", widgets = { "systemtray", "notifycenter", "system" } },
    },
}
