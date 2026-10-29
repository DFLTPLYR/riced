-- Shared commands. Bars live in Lua data; these handlers edit that data and
-- the next `view` declares the matching layers. No host bar command exists.
local state = {}

local function bar_options(args, adding)
	local options = {}
	for i = 1, #args, 2 do
		local key, value = args[i], args[i + 1]
		if not value or (key ~= "--anchor" and key ~= "--output" and not (adding and key == "--thickness")) then
			error("expected --anchor EDGE, --output NAME" .. (adding and " or --thickness PIXELS" or ""))
		end
		if options[key:sub(3)] ~= nil then
			error("duplicate option: " .. key)
		end
		options[key:sub(3)] = value
	end
	if not options.anchor then
		error("--anchor is required")
	end
	if not ({ top = true, bottom = true, left = true, right = true })[options.anchor] then
		error("--anchor must be top, bottom, left, or right")
	end
	if options.thickness then
		options.thickness = assert(tonumber(options.thickness), "thickness must be numeric")
		if options.thickness ~= math.floor(options.thickness) or options.thickness < 8 or options.thickness > 512 then
			error("thickness must be an integer in 8..512")
		end
	end
	return options
end

riced.ipc({
	app = function(self, args)
		return print("test")
	end,
	settings = function(self, args)
		if #args ~= 0 then
			error("usage: riced ipc settings")
		end
		self.settings_open = not self.settings_open
		self.menu = nil
	end,
	add = function(self, args)
		return self:add_bar(bar_options(args, true))
	end,
	remove = function(self, args)
		return self:remove_bar(bar_options(args, false))
	end,
})

return state
