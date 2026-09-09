import { useEffect, useRef } from 'react'
import { getToken } from '../api'

function liveWsUrl() {
  const proto = window.location.protocol === 'https:' ? 'wss:' : 'ws:'
  return `${proto}//${window.location.host}/api/ws`
}

const listeners = new Set()
let ws
let retryTimer
let attempt = 0
let closed = true

function dispatch(data) {
  for (const cb of [...listeners]) cb(data)
}

function disconnect() {
  closed = true
  clearTimeout(retryTimer)
  retryTimer = undefined
  attempt = 0
  if (ws) {
    ws.onclose = null
    ws.onerror = null
    ws.onmessage = null
    ws.close()
    ws = undefined
  }
}

function connect() {
  if (closed || ws) return
  const token = getToken()
  if (!token) return

  const socket = new WebSocket(liveWsUrl())
  ws = socket

  socket.onmessage = (ev) => {
    if (!ev.data || ev.data === 'ping') return
    try {
      const data = JSON.parse(ev.data)
      if (data.kind === 'hello') return
      dispatch(data)
    } catch {
      /* ignore malformed */
    }
  }

  socket.onopen = () => {
    attempt = 0
    socket.send(JSON.stringify({ token }))
  }

  socket.onclose = () => {
    if (ws === socket) ws = undefined
    if (closed || listeners.size === 0) return
    attempt += 1
    const delay = Math.min(15000, 1000 * 2 ** Math.min(attempt, 4))
    retryTimer = setTimeout(() => {
      retryTimer = undefined
      connect()
    }, delay)
  }
}

function subscribe(cb) {
  listeners.add(cb)
  if (listeners.size === 1) {
    closed = false
    connect()
  }
  return () => {
    listeners.delete(cb)
    if (listeners.size === 0) disconnect()
  }
}

/**
 * Subscribe to live updates over WebSocket (`GET /api/ws`).
 * JWT is sent as the first message, not on the URL.
 * Job/cluster changes and periodic `refresh` ticks (no client setInterval).
 * All hooks share one connection.
 */
export function useMgmtEvents(onEvent) {
  const cb = useRef(onEvent)
  cb.current = onEvent

  useEffect(() => subscribe((data) => cb.current?.(data)), [])
}

/** Refresh when a job/cluster event or server refresh tick arrives. */
export function useMgmtRefresh(refresh, { clusterId } = {}) {
  const pending = useRef(null)
  const refreshRef = useRef(refresh)
  refreshRef.current = refresh

  useMgmtEvents((ev) => {
    if (clusterId && ev.cluster_id && ev.cluster_id !== clusterId) return
    if (pending.current) return
    pending.current = setTimeout(() => {
      pending.current = null
      refreshRef.current?.()
    }, 200)
  })

  useEffect(
    () => () => {
      if (pending.current) clearTimeout(pending.current)
    },
    [],
  )
}
