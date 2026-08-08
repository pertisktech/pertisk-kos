const STORAGE_KEY = 'pertisk_kube_web_url'
const DEFAULT_URL = 'http://localhost:8091'

/**
 * Resolve pertisk-kube-web public URL.
 * Priority: browser override → server `KUBE_WEB_PUBLIC_URL` → Vite build env → localhost.
 */
export function kubeWebUrl(serverUrl) {
  try {
    const stored = localStorage.getItem(STORAGE_KEY)
    if (stored && stored.trim()) return normalize(stored)
  } catch {
    /* ignore */
  }
  if (typeof serverUrl === 'string' && serverUrl.trim()) return normalize(serverUrl)
  const env = import.meta.env.VITE_KUBE_WEB_URL
  if (typeof env === 'string' && env.trim()) return normalize(env)
  return DEFAULT_URL
}

function normalize(url) {
  return String(url).trim().replace(/\/$/, '')
}

export function setKubeWebUrl(url) {
  const next = normalize(url || '')
  try {
    if (!next || next === DEFAULT_URL) localStorage.removeItem(STORAGE_KEY)
    else localStorage.setItem(STORAGE_KEY, next)
  } catch {
    /* ignore */
  }
  return next || DEFAULT_URL
}

/** Hint for Linux service / reverse-proxy (not local make run). */
export function kubeWebServiceHint(kubeconfigFilename) {
  const name = kubeconfigFilename || '<cluster>.yaml'
  return [
    `# On the kube-web host (systemd), point KUBECONFIG at this cluster then restart:`,
    `#   sudo install -m 600 ~/Downloads/${name} /var/lib/pertisk-kube/kubeconfig`,
    `#   sudo systemctl restart pertisk-kube-web`,
    `# Then open the reverse-proxy URL (KUBE_WEB_PUBLIC_URL).`,
  ].join('\n')
}

export { DEFAULT_URL as KUBE_WEB_DEFAULT_URL, STORAGE_KEY as KUBE_WEB_STORAGE_KEY }
