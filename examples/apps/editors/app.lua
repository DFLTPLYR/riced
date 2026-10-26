-- Demonstrates the numeric stepper (`iced.spin_box`) driving a responsive card
-- layout (`iced.masonry`) in an ordinary framework app. The app owns the
-- values; the controls only emit numeric change events.
local app = { column_width = 140, spacing = 10 }

local CARDS = { "Alpha", "Beta", "Gamma", "Delta" }

function app:mount()
    self.changes = 0
end

function app:actions(event)
    if event.type ~= "action" then
        return
    end
    if event.key == "column_width" then
        self.column_width = event.value
    elseif event.key == "spacing" then
        self.spacing = event.value
    else
        return
    end
    self.changes = self.changes + 1
end

local function spin_row(label, value, min, max, step, key)
    return iced.row({
        iced.text(label):size(13):width("fill:7"):color(theme.on_surface),
        iced.spin_box(value, min, max):step(step):decimals(0):on_change(key):width(110),
    }):spacing(8)
end

function app:view()
    local cards = {}
    for index, name in ipairs(CARDS) do
        table.insert(cards, iced.container(iced.column({
            iced.text(name):color(theme.on_surface),
            iced.text(string.format("%d px", self.column_width)):size(11):color(theme.on_surface),
            iced.space():height(8 * index),
        }):spacing(4)):padding(12):width(size.fill)
            :background(theme.background):radius(8))
    end
    return iced.layer({
        id = "editors",
        layer = "top",
        anchors = { bottom = true, right = true },
        width = 320,
        height = 300,
        margin = { bottom = 16, right = 16 },
        content = iced.container(iced.column({
            iced.text("Layout editors"):size(15):color(theme.on_surface),
            iced.text("spin_box drives masonry; " .. self.changes .. " change(s)")
                :size(11):wrap("word"):color(theme.on_surface),
            spin_row("Column width", self.column_width, 100, 300, 20, "column_width"),
            spin_row("Spacing", self.spacing, 0, 32, 2, "spacing"),
            iced.scrollable(iced.masonry(cards):column_width(self.column_width)
                :spacing(self.spacing)):height(120),
        }):spacing(10)):padding(16):width(size.fill):height(size.fill)
            :background(theme.surface):radius(12),
    })
end

return app
