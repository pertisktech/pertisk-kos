const TOKEN_KEY = 'pertisk_mgmt_token'
const REMEMBER_KEY = 'pertisk_mgmt_remember'
const CREDS_KEY = 'pertisk_mgmt_creds'
const AUTH_PROVIDER_KEY = 'pertisk_mgmt_auth_provider'

export function getRememberMe() {
  const v = localStorage.getItem(REMEMBER_KEY)
  if (v === '1') return true
  if (v === '0') return false
  // Legacy installs kept the JWT in localStorage with no flag.
  return !!localStorage.getItem(TOKEN_KEY) || !!localStorage.getItem(CREDS_KEY)
}

export function setRememberMe(on) {
  localStorage.setItem(REMEMBER_KEY, on ? '1' : '0')
}

/** Saved local-login username/password when Remember password is on. */
export function getSavedCredentials() {
  try {
    const raw = localStorage.getItem(CREDS_KEY)
    if (!raw) return null
    const parsed = JSON.parse(raw)
    if (!parsed || typeof parsed !== 'object') return null
    return {
      username: typeof parsed.username === 'string' ? parsed.username : '',
      password: typeof parsed.password === 'string' ? parsed.password : '',
    }
  } catch {
    return null
  }
}

export function saveCredentials(username, password) {
  localStorage.setItem(
    CREDS_KEY,
    JSON.stringify({ username: username || '', password: password || '' }),
  )
}

export function clearSavedCredentials() {
  localStorage.removeItem(CREDS_KEY)
}

export function getAuthProvider() {
  return localStorage.getItem(AUTH_PROVIDER_KEY) || 'local'
}

export function setAuthProvider(provider) {
  localStorage.setItem(AUTH_PROVIDER_KEY, provider === 'auth0' ? 'auth0' : 'local')
}

export function clearAuthProvider() {
  localStorage.removeItem(AUTH_PROVIDER_KEY)
}

export function getToken() {
  return localStorage.getItem(TOKEN_KEY) || sessionStorage.getItem(TOKEN_KEY)
}

/**
 * Persist the JWT. When `remember` is true (default: last preference / localStorage),
 * keep the token across browser restarts; otherwise use sessionStorage only.
 */
export function setToken(token, remember = getRememberMe()) {
  setRememberMe(!!remember)
  if (remember) {
    localStorage.setItem(TOKEN_KEY, token)
    sessionStorage.removeItem(TOKEN_KEY)
  } else {
    sessionStorage.setItem(TOKEN_KEY, token)
    localStorage.removeItem(TOKEN_KEY)
  }
}

export function clearToken() {
  localStorage.removeItem(TOKEN_KEY)
  sessionStorage.removeItem(TOKEN_KEY)
}

/**
 * Sign out of the SPA. Auth0 users are redirected through `/api/auth/logout`
 * so the Auth0 (and federated IdP) SSO cookie is cleared — otherwise the next
 * "Continue with Auth0" silently reuses the previous account.
 */
export function logoutAndRedirect(provider = getAuthProvider()) {
  clearToken()
  clearAuthProvider()
  if (provider === 'auth0') {
    window.location.assign('/api/auth/logout')
    return
  }
  window.location.hash = '#/login'
}

export async function api(path, opts = {}) {
  const headers = { ...(opts.headers || {}) }
  if (opts.body && !(opts.body instanceof FormData)) {
    headers['Content-Type'] = 'application/json'
  }
  const token = getToken()
  if (token) headers.Authorization = `Bearer ${token}`

  const res = await fetch(`/api${path}`, {
    ...opts,
    headers,
    body: opts.body && !(opts.body instanceof FormData)
      ? JSON.stringify(opts.body)
      : opts.body,
  })

  if (res.status === 401) {
    clearToken()
    if (!path.startsWith('/auth/')) {
      window.location.hash = '#/login'
    }
    const err = await res.json().catch(() => ({ error: 'unauthorized' }))
    throw new Error(err.error || 'unauthorized')
  }

  const ct = res.headers.get('content-type') || ''
  if (!res.ok) {
    const err = ct.includes('json')
      ? await res.json().catch(() => ({ error: res.statusText }))
      : { error: await res.text() }
    throw new Error(err.error || res.statusText)
  }

  if (res.status === 204) return null
  if (ct.includes('json')) return res.json()
  return res.text()
}
