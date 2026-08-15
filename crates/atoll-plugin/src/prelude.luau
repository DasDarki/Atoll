-- The extension interface, built on top of the raw functions Atoll provides. Runs before the
-- extension itself and before the state is sandboxed.
local native = ...

local atoll = {}
local handlers = {}
local commands = {}

atoll.id = native.id
atoll.version = native.version

local function join(...)
	local parts = {}
	for index = 1, select("#", ...) do
		parts[index] = tostring((select(index, ...)))
	end
	return table.concat(parts, " ")
end

-- Hands the work to Atoll and waits for the answer without blocking the window.
local function await(pending)
	local ok, value = coroutine.yield({ __atoll_pending = pending })
	if not ok then
		error(value, 0)
	end
	return value
end

function atoll.log(...)
	native.log("info", join(...))
end

function atoll.warn(...)
	native.log("warn", join(...))
end

function atoll.error(...)
	native.log("error", join(...))
end

function atoll.on(event, handler)
	assert(type(event) == "string", "atoll.on braucht den Namen eines Ereignisses")
	assert(type(handler) == "function", "atoll.on braucht eine Funktion")
	local list = handlers[event]
	if not list then
		list = {}
		handlers[event] = list
	end
	table.insert(list, handler)
	return function()
		atoll.off(event, handler)
	end
end

function atoll.off(event, handler)
	local list = handlers[event]
	if not list then
		return
	end
	for index, candidate in ipairs(list) do
		if candidate == handler then
			table.remove(list, index)
			return
		end
	end
end

function atoll.sleep(milliseconds)
	return await(native.sleep(milliseconds))
end

atoll.ui = {
	toast = function(message)
		native.toast(tostring(message))
	end,
}

atoll.storage = {
	get = function(key)
		return native.storage_get(tostring(key))
	end,
	set = function(key, value)
		native.storage_set(tostring(key), value)
	end,
	remove = function(key)
		native.storage_set(tostring(key), nil)
	end,
	keys = function()
		return native.storage_keys()
	end,
}

atoll.tabs = {
	list = function()
		return native.tabs()
	end,
	current = function()
		for _, tab in ipairs(native.tabs()) do
			if tab.active then
				return tab
			end
		end
		return nil
	end,
	open = function(url, options)
		local background = false
		if type(options) == "table" and options.background then
			background = true
		end
		return native.open_tab(tostring(url), background)
	end,
	close = function(tab)
		native.close_tab(tab)
	end,
	activate = function(tab)
		native.activate_tab(tab)
	end,
}

atoll.commands = {
	register = function(command)
		assert(type(command) == "table", "atoll.commands.register braucht eine Tabelle")
		assert(type(command.id) == "string", "ein Befehl braucht eine id")
		assert(type(command.run) == "function", "ein Befehl braucht run")
		commands[command.id] = command.run
		native.register_command(command.id, tostring(command.title or command.id), command.shortcut)
	end,
	unregister = function(id)
		commands[id] = nil
		native.unregister_command(tostring(id))
	end,
}

local function fetch(method, url, options)
	options = if type(options) == "table" then options else {}
	return await(native.fetch(method, tostring(url), options.headers, options.body))
end

atoll.http = {
	request = fetch,
	get = function(url, options)
		return fetch("GET", url, options)
	end,
	post = function(url, options)
		return fetch("POST", url, options)
	end,
}

-- What Atoll calls into. Handlers run one after another, an error in one does not stop the rest.
local dispatch = {}

function dispatch.event(event, payload)
	local list = handlers[event]
	if not list then
		return
	end
	for _, handler in ipairs(table.clone(list)) do
		local ok, problem = pcall(handler, payload)
		if not ok then
			native.report(tostring(problem))
		end
	end
end

function dispatch.command(id)
	local run = commands[id]
	if not run then
		return
	end
	local ok, problem = pcall(run)
	if not ok then
		native.report(tostring(problem))
	end
end

return atoll, dispatch
