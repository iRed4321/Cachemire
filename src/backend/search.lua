-- The global search's server-side prefilter (see `read_entries_lua` in explorer.rs).
--
-- KEYS: the keys to look at.
-- ARGV[1]: the word searched for, already lowercased by the caller.
-- ARGV[2]: how many entries of a key to read.
--
-- Answers, for each key that could hold a hit, {key, type, status, entries}, where
-- `entries` lists field, value, field, value... of the entries that might match.
-- It never leaves out a real hit: the caller checks what it gets exactly, this only
-- spares it the entries that certainly don't match. Text with a non-ASCII byte in
-- it is always passed on, since only the caller lowercases Unicode the way it
-- searches. status 1 means "a stream: read it yourself".
--
-- It only reads. Keys that vanished meanwhile are left out.

local needle = ARGV[1]
local limit = tonumber(ARGV[2])
local find, lower = string.find, string.lower
local NON_ASCII = '[\128-\255]'
local ascii_needle = not find(needle, NON_ASCII)

-- false only when `text` certainly doesn't hold the word: an all-ASCII text is
-- lowercased the same way here as by the caller, and a word with non-ASCII
-- letters can't be in it
local function may_match(text)
  if find(text, NON_ASCII) then
    return true
  end
  return ascii_needle and find(lower(text), needle, 1, true) ~= nil
end

local out = {}
for i = 1, #KEYS do
  local key = KEYS[i]
  local kind = redis.call('TYPE', key)['ok']
  local status = 0
  local entries = {}

  local function keep(field, value)
    if may_match(field) or may_match(value) then
      entries[#entries + 1] = field
      entries[#entries + 1] = value
    end
  end

  if kind == 'string' then
    local value = redis.call('GET', key)
    if value then
      keep('', value)
    end
  elseif kind == 'hash' then
    local flat = redis.call('HSCAN', key, 0, 'COUNT', limit)[2]
    for j = 1, #flat, 2 do
      keep(flat[j], flat[j + 1])
    end
  elseif kind == 'list' then
    local items = redis.call('LRANGE', key, 0, limit - 1)
    for j = 1, #items do
      keep('[' .. (j - 1) .. ']', items[j])
    end
  elseif kind == 'set' then
    local members = redis.call('SSCAN', key, 0, 'COUNT', limit)[2]
    for j = 1, #members do
      keep('', members[j])
    end
  elseif kind == 'zset' then
    -- (a sorted set's entries are its scores, then its members)
    local flat = redis.call('ZRANGE', key, 0, limit - 1, 'WITHSCORES')
    for j = 1, #flat, 2 do
      keep(flat[j + 1], flat[j])
    end
  elseif kind == 'stream' then
    status = 1
  elseif kind == 'none' then
    status = -1
  end

  if status == 1 or (status == 0 and (#entries > 0 or may_match(key))) then
    out[#out + 1] = { key, kind, status, entries }
  end
end
return out
