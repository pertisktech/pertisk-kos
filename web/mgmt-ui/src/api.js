const TOKEN_KEY = 'pertisk_mgmt_token'

export function getToken() {
  return localStorage.getItem(TOKEN_KEY)
}

export function setToken(token) {
  localStorage.setItem(TOKEN_KEY, token)
}

export function clearToken() {
  localStorage.removeItem(TOKEN_KEY)
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
