import { useEffect, useRef, useState } from 'react'
import { Icon } from '../../components/Icons'
import { buildPodLogsWsUrl } from './api'

export default function PodLogsPanel({
  clusterId,
  namespace,
  name,
  containers = [],
  onClose,
}) {
  const [container, setContainer] = useState(containers[0] || '')
  const [text, setText] = useState('')
  const [status, setStatus] = useState('connecting')
  const preRef = useRef(null)
  const wsRef = useRef(null)

  useEffect(() => {
    if (!clusterId || !namespace || !name) return undefined
    setText('')
    setStatus('connecting')
    const url = buildPodLogsWsUrl(clusterId, namespace, name, {
      container: container || undefined,
      follow: true,
      tail: 300,
    })
    const ws = new WebSocket(url)
    wsRef.current = ws
    ws.onopen = () => setStatus('live')
    ws.onmessage = (ev) => {
      const chunk = typeof ev.data === 'string' ? ev.data : new TextDecoder().decode(ev.data)
      setText((t) => {
        const next = t + chunk
        return next.length > 400_000 ? next.slice(-350_000) : next
      })
      requestAnimationFrame(() => {
        if (preRef.current) preRef.current.scrollTop = preRef.current.scrollHeight
      })
    }
    ws.onerror = () => setStatus('error')
    ws.onclose = () => setStatus((s) => (s === 'error' ? s : 'closed'))
    return () => {
      try {
        ws.close()
      } catch {
        /* ignore */
      }
      wsRef.current = null
    }
  }, [clusterId, namespace, name, container])

  return (
    <div className="pod-logs-panel">
      <div className="pod-logs-bar">
        <div className="pod-logs-meta">
          <Icon name="logs" size={14} />
          <span className="mono-inline">
            {namespace}/{name}
          </span>
          <span className={`badge ${status === 'live' ? 'ready' : status === 'error' ? 'error' : ''}`}>
            {status}
          </span>
        </div>
        <div className="pod-logs-actions">
          {containers.length > 1 && (
            <select value={container} onChange={(e) => setContainer(e.target.value)}>
              {containers.map((c) => (
                <option key={c} value={c}>
                  {c}
                </option>
              ))}
            </select>
          )}
          <button type="button" className="secondary btn-icon" onClick={onClose}>
            <Icon name="x" size={14} /> Close
          </button>
        </div>
      </div>
      <pre ref={preRef} className="pod-logs-body color-log">
        {text || (status === 'connecting' ? 'Connecting…' : '')}
      </pre>
    </div>
  )
}
