import { useEffect, useRef } from 'react'
import { getToken } from '../api'

/**
 * Subscribe to management SSE (`GET /api/events?token=`).
 * Calls `onEvent` for job/cluster updates (not keep-alive pings / hello).
 */
export function useMgmtEvents(onEvent) {
  const cb = useRef(onEvent)
  cb.current = onEvent

  useEffect(() => {
    const token = getToken()
    if (!token) return undefined

    let es
    let closed = false
    let retryTimer

    function connect() {
      if (closed) return
      es = new EventSource(`/api/events?token=${encodeURIComponent(token)}`)

      const handle = (ev) => {
        if (!ev.data || ev.data === 'ping') return
        try {
          const data = JSON.parse(ev.data)
          if (data.kind === 'hello') return
          cb.current?.(data)
        } catch {
          /* ignore malformed */
        }
      }

      es.addEventListener('job', handle)
      es.addEventListener('cluster', handle)
      es.onmessage = handle

      es.onerror = () => {
        // CLOSED = server rejected / network drop; retry with backoff.
        if (closed || es.readyState !== EventSource.CLOSED) return
        es.close()
        retryTimer = setTimeout(connect, 3000)
      }
    }

    connect()

    return () => {
      closed = true
      clearTimeout(retryTimer)
      if (es) es.close()
    }
  }, [])
}

/** Refresh when any job/cluster event arrives (optional cluster_id filter). */
export function useMgmtRefresh(refresh, { clusterId } = {}) {
  useMgmtEvents((ev) => {
    if (clusterId && ev.cluster_id && ev.cluster_id !== clusterId) return
    refresh()
  })
}
