import { useEffect, useRef } from 'react'
import { Terminal as XTerm } from '@xterm/xterm'
import { FitAddon } from '@xterm/addon-fit'
import { WebLinksAddon } from '@xterm/addon-web-links'
import '@xterm/xterm/css/xterm.css'
import { buildHostShellWsUrl } from '../pages/cluster-k8s/api'

/**
 * Host OS shell on the management server (KUBECONFIG set for this cluster).
 */
export default function Xterm({ clusterId, clusterName, onClose }) {
  const terminalRef = useRef(null)
  const xtermRef = useRef(null)
  const wsRef = useRef(null)
  const fitAddonRef = useRef(null)
  const lastDims = useRef(null)
  const resizeTimer = useRef(null)

  useEffect(() => {
    if (!terminalRef.current || !clusterId) return undefined

    const style = getComputedStyle(document.documentElement)
    const bg = style.getPropertyValue('--bg-elevated').trim() || '#131421'
    const fg = style.getPropertyValue('--text').trim() || '#e8e8e9'

    const xterm = new XTerm({
      cursorBlink: true,
      fontSize: 13,
      fontFamily: 'ui-monospace, SFMono-Regular, Menlo, Consolas, monospace',
      theme: {
        background: bg,
        foreground: fg,
        cursor: fg,
      },
    })
    const fit = new FitAddon()
    xterm.loadAddon(fit)
    xterm.loadAddon(new WebLinksAddon())
    xterm.open(terminalRef.current)
    fit.fit()
    xtermRef.current = xterm
    fitAddonRef.current = fit

    const wsUrl = buildHostShellWsUrl(clusterId)
    const ws = new WebSocket(wsUrl)
    wsRef.current = ws

    const sendResize = () => {
      if (!wsRef.current || wsRef.current.readyState !== WebSocket.OPEN || !xtermRef.current) return
      const cols = xtermRef.current.cols
      const rows = xtermRef.current.rows
      const last = lastDims.current
      if (last && last.cols === cols && last.rows === rows) return
      lastDims.current = { cols, rows }
      wsRef.current.send(JSON.stringify({ type: 'resize', cols, rows }))
    }

    ws.onopen = () => {
      sendResize()
    }
    ws.onmessage = (ev) => {
      xterm.write(typeof ev.data === 'string' ? ev.data : new TextDecoder().decode(ev.data))
    }
    ws.onerror = () => {
      xterm.writeln('\r\n\x1b[1;31mWebSocket error\x1b[0m')
    }
    ws.onclose = () => {
      xterm.writeln('\r\n\x1b[90mdisconnected\x1b[0m')
    }

    const dataDisp = xterm.onData((data) => {
      if (ws.readyState === WebSocket.OPEN) ws.send(data)
    })

    const onWinResize = () => {
      if (resizeTimer.current) window.clearTimeout(resizeTimer.current)
      resizeTimer.current = window.setTimeout(() => {
        fitAddonRef.current?.fit()
        sendResize()
      }, 80)
    }
    window.addEventListener('resize', onWinResize)

    return () => {
      window.removeEventListener('resize', onWinResize)
      if (resizeTimer.current) window.clearTimeout(resizeTimer.current)
      dataDisp.dispose()
      try {
        ws.close()
      } catch {
        /* ignore */
      }
      xterm.dispose()
      xtermRef.current = null
      wsRef.current = null
    }
  }, [clusterId])

  return (
    <div className="xterm-shell">
      <div className="xterm-shell-bar">
        <span className="mono-inline">
          host shell{clusterName ? ` · ${clusterName}` : ''} · kubectl / helm
        </span>
        {onClose && (
          <button type="button" className="secondary btn-icon" onClick={onClose}>
            Close
          </button>
        )}
      </div>
      <div className="xterm-shell-body xterm-shell-body-tall" ref={terminalRef} />
    </div>
  )
}
