export function applyTheme(theme) {
  const t = theme === 'light' ? 'light' : 'dark'
  const root = document.documentElement
  root.setAttribute('data-theme', t)
  root.classList.toggle('dark', t === 'dark')
  root.classList.toggle('light', t === 'light')
  localStorage.setItem('theme', t)
}

export function initTheme() {
  applyTheme(localStorage.getItem('theme') || 'dark')
}
