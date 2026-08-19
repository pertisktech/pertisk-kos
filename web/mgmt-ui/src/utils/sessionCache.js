/** Last-good JSON for a tab so remounts are not a blank loading page. */

export function readSessionJson(key, fallback) {
  try {
    const raw = sessionStorage.getItem(key)
    if (!raw) return fallback
    return JSON.parse(raw)
  } catch {
    return fallback
  }
}

export function writeSessionJson(key, value) {
  try {
    sessionStorage.setItem(key, JSON.stringify(value))
  } catch {
    /* quota / private mode */
  }
}
