import { useCallback, useEffect, useRef, useState } from 'react'
import { Icon } from './Icons'
import Xterm from './Xterm'
import { useShellDock } from '../shell/ShellDockContext'

/**
 * Bottom multi-tab shell dock: management host (pertiskctl) + per-cluster kubectl shells.
 */
export default function BottomShell() {
  const {
    open,
    setOpen,
    height,
    setHeight,
    tabs,
    activeId,
    setActiveId,
    openMgmtShell,
    newMgmtTab,
    closeTab,
  } = useShellDock()
  const dragRef = useRef(null)
  const [dragging, setDragging] = useState(false)
  const [mounted, setMounted] = useState(() => new Set([activeId]))

  useEffect(() => {
    setMounted((prev) => {
      if (prev.has(activeId)) return prev
      const next = new Set(prev)
      next.add(activeId)
      return next
    })
  }, [activeId])

  useEffect(() => {
    const ids = new Set(tabs.map((t) => t.id))
    setMounted((prev) => {
      let changed = false
      const next = new Set()
      for (const id of prev) {
        if (ids.has(id)) next.add(id)
        else changed = true
      }
      return changed ? next : prev
    })
  }, [tabs])

  const onPointerDown = useCallback(
    (e) => {
      if (!open) return
      e.preventDefault()
      dragRef.current = { startY: e.clientY, startH: height }
      setDragging(true)
    },
    [open, height],
  )

  useEffect(() => {
    if (!dragging) return undefined
    function onMove(e) {
      const d = dragRef.current
      if (!d) return
      const next = Math.min(
        Math.max(d.startH + (d.startY - e.clientY), 160),
        Math.floor(window.innerHeight * 0.7),
      )
      setHeight(next)
    }
    function onUp() {
      setDragging(false)
      dragRef.current = null
    }
    window.addEventListener('pointermove', onMove)
    window.addEventListener('pointerup', onUp)
    return () => {
      window.removeEventListener('pointermove', onMove)
      window.removeEventListener('pointerup', onUp)
    }
  }, [dragging, setHeight])

  const canClose = tabs.length > 1

  return (
    <div className={`bottom-shell${open ? ' open' : ''}${dragging ? ' dragging' : ''}`}>
      <div className="bottom-shell-bar">
        {open ? (
          <button
            type="button"
            className="bottom-shell-resize"
            aria-label="Resize shell"
            onPointerDown={onPointerDown}
          />
        ) : null}
        <div className="bottom-shell-tabs" role="tablist" aria-label="Shell sessions">
          {tabs.map((tab) => (
            <button
              key={tab.id}
              type="button"
              role="tab"
              aria-selected={tab.id === activeId}
              className={`bottom-shell-tab${tab.id === activeId ? ' active' : ''}`}
              onClick={() => {
                setActiveId(tab.id)
                setOpen(true)
              }}
            >
              <Icon name={tab.kind === 'mgmt' ? 'settings' : 'clusters'} size={12} />
              <span>{tab.title}</span>
              {canClose && (
                <span
                  className="bottom-shell-tab-close"
                  role="presentation"
                  onClick={(e) => {
                    e.stopPropagation()
                    closeTab(tab.id)
                  }}
                >
                  <Icon name="x" size={12} />
                </span>
              )}
            </button>
          ))}
          <button
            type="button"
            className="bottom-shell-tab-add"
            title="New shell tab"
            aria-label="New shell tab"
            onClick={newMgmtTab}
          >
            <Icon name="plus" size={14} />
          </button>
        </div>
        <div className="bottom-shell-actions">
          <button
            type="button"
            className="theme-toggle"
            title="Focus pertiskctl shell"
            aria-label="Open management shell"
            onClick={openMgmtShell}
          >
            <Icon name="terminal" size={14} />
          </button>
          <button
            type="button"
            className="theme-toggle"
            title={open ? 'Collapse shell' : 'Expand shell'}
            aria-label={open ? 'Collapse shell' : 'Expand shell'}
            onClick={() => setOpen((v) => !v)}
          >
            <Icon name={open ? 'chevron-down' : 'chevron-up'} size={14} />
          </button>
        </div>
      </div>
      <div
        className="bottom-shell-body"
        style={{ height: open ? height : 0, display: open ? undefined : 'none' }}
        aria-hidden={!open}
      >
        {tabs.map((tab) =>
          mounted.has(tab.id) ? (
            <div
              key={tab.id}
              className={`bottom-shell-pane${tab.id === activeId ? ' active' : ''}`}
              hidden={tab.id !== activeId}
            >
              {tab.kind === 'mgmt' ? (
                <Xterm kind="mgmt" bare />
              ) : (
                <Xterm kind="cluster" clusterId={tab.clusterId} clusterName={tab.title} bare />
              )}
            </div>
          ) : null,
        )}
      </div>
    </div>
  )
}
