/** Parse RFC3339 (including chrono nanoseconds) into a local Date. */
export function parseDate(iso) {
  if (!iso) return null
  const raw = String(iso).trim()
  if (!raw) return null
  let d = new Date(raw)
  if (!Number.isNaN(d.getTime())) return d
  const trimmed = raw.replace(/(\.\d{3})\d+/, '$1')
  d = new Date(trimmed)
  return Number.isNaN(d.getTime()) ? null : d
}

export function formatDate(iso) {
  const d = parseDate(iso)
  if (!d) return iso || '—'
  return new Intl.DateTimeFormat(undefined, {
    day: 'numeric',
    month: 'short',
    year: 'numeric',
  }).format(d)
}

export function formatTime(iso) {
  const d = parseDate(iso)
  if (!d) return ''
  return new Intl.DateTimeFormat(undefined, {
    hour: '2-digit',
    minute: '2-digit',
    second: '2-digit',
  }).format(d)
}

export function formatDateTime(iso) {
  const d = parseDate(iso)
  if (!d) return iso || '—'
  const date = formatDate(iso)
  const time = formatTime(iso)
  return time ? `${date}, ${time}` : date
}

export function formatDuration(ms) {
  if (ms == null || Number.isNaN(ms) || ms < 0) return '—'
  const sec = Math.floor(ms / 1000)
  if (sec < 1) return '<1s'
  const days = Math.floor(sec / 86400)
  const hours = Math.floor((sec % 86400) / 3600)
  const mins = Math.floor((sec % 3600) / 60)
  const secs = sec % 60
  if (days > 0) return hours ? `${days}d ${hours}h` : `${days}d`
  if (hours > 0) return mins ? `${hours}h ${mins}m` : `${hours}h`
  if (mins > 0) return secs ? `${mins}m ${secs}s` : `${mins}m`
  return `${secs}s`
}
