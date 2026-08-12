import { useEffect, useMemo, useRef } from 'react'

/**
 * Keyword → color map (order = precedence), inspired by
 * https://github.com/floriankraft/color-logviewer
 *
 * Whole-line mode (default): first matching keyword colors the entire line.
 * Word mode (`wordMode`): only the matched keyword is colored.
 */
export const DEFAULT_COLOR_MAP = [
  { keyword: 'ERROR', color: 'red' },
  { keyword: 'error:', color: 'red' },
  { keyword: 'FAILED', color: 'red' },
  { keyword: 'failed', color: 'red' },
  { keyword: 'FATAL', color: 'red' },
  { keyword: 'WARN', color: 'yellow' },
  { keyword: 'warn:', color: 'yellow' },
  { keyword: 'WARNING', color: 'yellow' },
  { keyword: 'DEBUG', color: 'green' },
  { keyword: 'TRACE', color: 'blue' },
  { keyword: 'INFO', color: 'cyan' },
  { keyword: 'complete', color: 'green' },
  { keyword: 'succeeded', color: 'green' },
]

const COLOR_CLASS = {
  red: 'log-sev-red',
  yellow: 'log-sev-yellow',
  green: 'log-sev-green',
  blue: 'log-sev-blue',
  cyan: 'log-sev-cyan',
  magenta: 'log-sev-magenta',
}

function escapeRegExp(s) {
  return s.replace(/[.*+?^${}()|[\]\\]/g, '\\$&')
}

/** Split a line into plain / highlighted spans (word mode). */
function colorizeWords(line, keyword, color) {
  const re = new RegExp(`(${escapeRegExp(keyword)})`, 'g')
  const parts = line.split(re)
  const cls = COLOR_CLASS[color]
  return parts.map((text, i) =>
    text === keyword && cls ? (
      <span key={i} className={`log-kw ${cls}`}>{text}</span>
    ) : (
      <span key={i}>{text}</span>
    ),
  )
}

/**
 * @param {string} line
 * @param {{ keyword: string, color: string }[]} colorMap
 * @param {boolean} wordMode
 * @returns {{ className: string, content: import('react').ReactNode }}
 */
export function colorizeLine(line, colorMap = DEFAULT_COLOR_MAP, wordMode = false) {
  for (const { keyword, color } of colorMap) {
    if (!keyword || !line.includes(keyword)) continue
    const cls = COLOR_CLASS[color] || ''
    if (wordMode) {
      return { className: 'log-line', content: colorizeWords(line, keyword, color) }
    }
    return { className: `log-line ${cls}`.trim(), content: line }
  }
  return { className: 'log-line', content: line }
}

/**
 * Colored log pane with optional follow-scroll.
 *
 * @param {object} props
 * @param {string} props.text
 * @param {string} [props.empty]
 * @param {boolean} [props.follow]
 * @param {(v: boolean) => void} [props.onFollowChange]
 * @param {boolean} [props.wordMode] color only keywords (color-logviewer `-s`)
 * @param {{ keyword: string, color: string }[]} [props.colorMap]
 * @param {string} [props.className]
 * @param {string} [props['aria-label']]
 */
export default function ColorLogViewer({
  text = '',
  empty = '—',
  follow = true,
  onFollowChange,
  wordMode = false,
  colorMap = DEFAULT_COLOR_MAP,
  className = '',
  'aria-label': ariaLabel = 'Log output',
}) {
  const ref = useRef(null)
  const lines = useMemo(() => {
    if (!text) return null
    // Keep a trailing empty line so final `\n` still renders as a blank row.
    return text.endsWith('\n') ? text.slice(0, -1).split('\n') : text.split('\n')
  }, [text])

  useEffect(() => {
    if (!follow || !ref.current) return
    ref.current.scrollTop = ref.current.scrollHeight
  }, [text, follow])

  function onScroll() {
    const el = ref.current
    if (!el || !onFollowChange) return
    const atBottom = el.scrollHeight - el.scrollTop - el.clientHeight < 48
    if (atBottom && !follow) onFollowChange(true)
    if (!atBottom && follow) onFollowChange(false)
  }

  return (
    <pre
      ref={ref}
      className={`log-box mono color-log-viewer ${className}`.trim()}
      onScroll={onScroll}
      aria-label={ariaLabel}
    >
      {lines == null ? (
        empty
      ) : (
        lines.map((line, i) => {
          const { className: lineCls, content } = colorizeLine(line, colorMap, wordMode)
          return (
            <div key={i} className={lineCls}>
              {content || '\u00a0'}
            </div>
          )
        })
      )}
    </pre>
  )
}
