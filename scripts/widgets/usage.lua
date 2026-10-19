-- CPU / RAM / GPU line graph. History is sampled once per STEP seconds on the
-- host's monotonic `system.sampled` clock, so the redraw cadence (popups tick
-- far faster than the sample rate) never distorts the series.
local WINDOW = 60
local STEP = 1.0
local MAX = 600

local cpu, ram, gpu, last, gpu_ok = {}, {}, {}, nil, true

local function push(values, value)
    values[#values + 1] = math.max(0, math.min(100, value or 0))
    while #values > MAX do
        table.remove(values, 1)
    end
end

local function sample()
    local at = system.sampled or 0
    if last and at - last < STEP then
        return
    end
    last = at
    push(cpu, system.cpu_usage)
    push(ram, system.mem_usage)
    if system.gpu_usage ~= nil then
        gpu_ok = true
        push(gpu, system.gpu_usage)
    else
        gpu_ok = false
        gpu = {}
    end
end

local function series()
    local out = {
        { values = cpu, color = theme.primary, width = 1.6 },
        { values = ram, color = theme.secondary, width = 1.6 },
    }
    if gpu_ok and #gpu > 1 then
        out[#out + 1] = { values = gpu, color = theme.warning, width = 1.6 }
    end
    return out
end

local function legend()
    local function chip(label, color, value)
        return iced.row({
            iced.text("●"):size(10):color(color),
            iced.text(string.format("%s %.0f%%", label, value or 0)):size(11):color(color),
        })
    end
    local row = {
        chip("CPU", theme.primary, system.cpu_usage),
        chip("RAM", theme.secondary, system.mem_usage),
    }
    if gpu_ok then
        row[#row + 1] = chip("GPU", theme.warning, system.gpu_usage)
    else
        row[#row + 1] = iced.text("GPU N/A"):size(11)
    end
    return iced.row(row):spacing(10)
end

local app = { defaults = { interval = 1, size = 13 } }

function app:view(props)
    sample()
    props = props or {}
    return iced.graph({
        width = props.width or 160,
        height = props.height or 22,
        window = WINDOW,
        series = series(),
    })
end

function app:popup()
    sample()
    return {
        ui = iced.card({
            title = "Usage",
            icon = "cpu",
            body = iced.column({
                iced.graph({ width = 268, height = 100, window = WINDOW, series = series() }),
                legend(),
            }):spacing(8),
        }),
        width = 300,
    }
end

function app:actions(events)
    if events.type == "click" and events.button == "left" then
        self:popup()
    end
end

return app
