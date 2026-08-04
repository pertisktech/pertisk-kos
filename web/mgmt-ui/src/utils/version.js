/** App version from build-time env (VITE_APP_VERSION / VERSION). */
export function getAppVersion() {
  try {
    let version = import.meta.env.VITE_APP_VERSION
    if (version && String(version).trim()) {
      version = String(version).trim().replace(/^v+/, '')
      if (version.length > 0) return version
    }
    return '0.1.0'
  } catch {
    return '0.1.0'
  }
}

export const APP_VERSION = getAppVersion()
