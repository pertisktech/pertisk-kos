import { useEffect, useMemo, useRef } from 'react'

/**
 * Keyword → color map (order = precedence). Used by `wordMode`.
 * Default rendering is node-style token color (timestamp, level, kv, strings, ANSI).
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

const ANSI_FG = {
  30: 'black',
  31: 'red',
  32: 'green',
  33: 'yellow',
  34: 'blue',
  35: 'magenta',
  36: 'cyan',
  37: 'white',
  90: 'black',
  91: 'red',
  92: 'green',
  93: 'yellow',
  94: 'blue',
  95: 'magenta',
  96: 'cyan',
  97: 'white',
}

const LEVEL_CLASS = {
  ERROR: 'log-sev-red',
  FATAL: 'log-sev-red',
  FAILED: 'log-sev-red',
  FAIL: 'log-sev-red',
  WARN: 'log-sev-yellow',
  WARNING: 'log-sev-yellow',
  INFO: 'log-sev-cyan',
  DEBUG: 'log-sev-green',
  TRACE: 'log-sev-blue',
  SUCCESS: 'log-sev-green',
  SUCCEEDED: 'log-sev-green',
  COMPLETE: 'log-sev-green',
  OK: 'log-sev-green',
}

const TS_ISO_RE = /^(\d{4}-\d{2}-\d{2}[T ]\d{2}:\d{2}:\d{2}(?:\.\d+)?(?:Z|[+-]\d{2}:?\d{2})?)\s+/
const TS_SYSLOG_RE = /^([A-Z][a-z]{2}\s+\d{1,2}\s+\d{2}:\d{2}:\d{2})\s+/
const LEVEL_HEAD_RE = /^(ERROR|FATAL|WARN(?:ING)?|INFO|DEBUG|TRACE|FAILED|FAIL)(:|\b)(\s*)/i
const LEVEL_HEAD_ALT_RE = /^(error|fatal|warn(?:ing)?|failed)(:|\b)(\s*)/
const REMAINDER_RE =
  /("[^"\\]*(?:\\.[^"\\]*)*"|'[^'\\]*(?:\\.[^'\\]*)*'|\b(?:ERROR|FATAL|WARN(?:ING)?|INFO|DEBUG|TRACE|FAILED|SUCCESS|SUCCEEDED|COMPLETE)\b|\b(?:error|fatal|warn(?:ing)?|failed):|\b(?:complete|succeeded)\b|[A-Za-z_][\w.-]*=[^\s]+)/g

function escapeRegExp(s) {
  return s.replace(/[.*+?^${}()|[\]\\]/g, '\\$&')
}

function hasAnsi(s) {
  return s.includes('\u001b') || s.includes('\u009b')
}

function ansiClass(state) {
  const bits = []
  if (state.fg) bits.push(`log-ansi-${state.fg}`)
  if (state.bold) bits.push('log-kw')
  if (state.dim) bits.push('log-ts')
  return bits.join(' ')
}

function applySgr(state, params) {
  const codes = params.length ? params : [0]
  for (const n of codes) {
    if (n === 0) {
      state.fg = null
      state.bold = false
      state.dim = false
    } else if (n === 1) {
      state.bold = true
      state.dim = false
    } else if (n === 2) {
      state.dim = true
    } else if (n === 22) {
      state.bold = false
      state.dim = false
    } else if (n === 39) {
      state.fg = null
    } else if (ANSI_FG[n]) {
      state.fg = ANSI_FG[n]
    }
  }
}

/** Render a line that contains ANSI SGR (Node / chalk / script colors). */
function renderAnsi(line) {
  const out = []
  const state = { fg: null, bold: false, dim: false }
  let buf = ''
  let key = 0

  const flush = () => {
    if (!buf) return
    const cls = ansiClass(state)
    out.push(
      cls ? (
        <span key={key} className={cls}>{buf}</span>
      ) : (
        <span key={key}>{buf}</span>
      ),
    )
    key += 1
    buf = ''
  }

  for (let i = 0; i < line.length; i += 1) {
    const c = line[i]
    if (c === '\r') continue
    if (c !== '\u001b' && c !== '\u009b') {
      buf += c
      continue
    }
    const csi = c === '\u009b' || line[i + 1] === '['
    if (c === '\u001b' && line[i + 1] !== '[') {
      i += 1
      continue
    }
    if (!csi) continue
    flush()
    let j = c === '\u009b' ? i + 1 : i + 2
    let params = ''
    while (j < line.length) {
      const ch = line[j]
      if (ch >= '@' && ch <= '~') {
        if (ch === 'm') {
          applySgr(
            state,
            params
              .split(';')
              .filter(Boolean)
              .map((x) => Number(x) || 0),
          )
        }
        i = j
        break
      }
      params += ch
      j += 1
    }
  }
  flush()
  return out
}

function levelClass(word) {
  return LEVEL_CLASS[word.toUpperCase().replace(/:$/, '')] || ''
}

function lineTone(line) {
  if (/\b(ERROR|FATAL|FAILED)\b|error:|fatal:/i.test(line)) return 'log-line-err'
  if (/\bWARN(?:ING)?\b|warn:/i.test(line)) return 'log-line-warn'
  return ''
}

function highlightRemainder(text, startKey = 0) {
  if (!text) return []
  const parts = []
  let last = 0
  let key = startKey
  REMAINDER_RE.lastIndex = 0
  let m = REMAINDER_RE.exec(text)
  while (m) {
    if (m.index > last) {
      parts.push(<span key={key}>{text.slice(last, m.index)}</span>)
      key += 1
    }
    const tok = m[0]
    if (tok.startsWith('"') || tok.startsWith("'")) {
      parts.push(<span key={key} className="log-str">{tok}</span>)
    } else if (tok.includes('=') && !tok.startsWith('error') && !tok.startsWith('warn')) {
      const eq = tok.indexOf('=')
      parts.push(
        <span key={key}>
          <span className="log-key">{tok.slice(0, eq)}</span>
          <span className="log-eq">=</span>
          <span className="log-val">{tok.slice(eq + 1)}</span>
        </span>,
      )
    } else {
      const cls = levelClass(tok)
      parts.push(
        cls ? (
          <span key={key} className={`log-kw ${cls}`}>{tok}</span>
        ) : (
          <span key={key}>{tok}</span>
        ),
      )
    }
    key += 1
    last = m.index + tok.length
    m = REMAINDER_RE.exec(text)
  }
  if (last < text.length) {
    parts.push(<span key={key}>{text.slice(last)}</span>)
  }
  return parts
}

/** Node / tracing compact: timestamp, level, then highlighted message. */
function renderNodeStyle(line) {
  const parts = []
  let rest = line
  let key = 0

  const syslog = rest.match(TS_SYSLOG_RE)
  const iso = syslog ? null : rest.match(TS_ISO_RE)
  const tsMatch = syslog || iso
  if (tsMatch) {
    parts.push(<span key={key} className="log-ts">{tsMatch[1]} </span>)
    key += 1
    rest = rest.slice(tsMatch[0].length)
  }

  if (rest.startsWith('$ ') || rest.startsWith('$')) {
    const dollar = rest.startsWith('$ ') ? '$ ' : '$'
    parts.push(<span key={key} className="log-cmd">{dollar}</span>)
    key += 1
    rest = rest.slice(dollar.length)
    parts.push(<span key={key} className="log-str">{rest}</span>)
    return parts
  }

  if (rest.startsWith('==> ')) {
    parts.push(<span key={key} className="log-cmd">{'==> '}</span>)
    key += 1
    rest = rest.slice(4)
  }

  const head = rest.match(LEVEL_HEAD_RE) || rest.match(LEVEL_HEAD_ALT_RE)
  if (head) {
    const word = head[1] + (head[2] === ':' ? ':' : '')
    const cls = levelClass(head[1])
    parts.push(<span key={key} className={`log-kw ${cls}`}>{word}</span>)
    key += 1
    if (head[2] !== ':') parts.push(<span key={key}>{head[3] || ' '}</span>)
    else if (head[3]) parts.push(<span key={key}>{head[3]}</span>)
    key += 1
    rest = rest.slice(head[0].length)
  }

  return parts.concat(highlightRemainder(rest, key))
}

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
  if (wordMode) {
    for (const { keyword, color } of colorMap) {
      if (!keyword || !line.includes(keyword)) continue
      return { className: 'log-line', content: colorizeWords(line, keyword, color) }
    }
    return { className: 'log-line', content: line }
  }

  const tone = lineTone(line)
  const className = ['log-line', tone].filter(Boolean).join(' ')
  if (hasAnsi(line)) {
    return { className, content: renderAnsi(line) }
  }
  return { className, content: renderNodeStyle(line) }
}

/**
 * Colored log pane with optional follow-scroll.
 * Default coloring matches node logs (tracing / journal / chalk ANSI).
 *
 * @param {object} props
 * @param {string} props.text
 * @param {string} [props.empty]
 * @param {boolean} [props.follow]
 * @param {(v: boolean) => void} [props.onFollowChange]
 * @param {boolean} [props.wordMode] color only keywords (legacy color-logviewer `-s`)
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
