import { useEffect, useRef } from 'react'
import { ensureMonacoYaml, monaco, monacoThemeFromDom } from '../monaco/setup'

const SCHEMA_SUFFIX = {
  machine: 'machine.yaml',
  kubeconfig: 'kubeconfig.yaml',
}

/**
 * Monaco YAML editor with schema-aware completion / hover / validation.
 *
 * @param {object} props
 * @param {string} props.value
 * @param {(next: string) => void} [props.onChange]
 * @param {'machine' | 'kubeconfig'} [props.schema]
 * @param {boolean} [props.readOnly]
 * @param {string} [props.path] unique URI stem (cluster id, template id, …)
 * @param {string} [props.className]
 */
export default function YamlEditor({
  value = '',
  onChange,
  schema = 'machine',
  readOnly = false,
  path = 'untitled',
  className = '',
}) {
  const hostRef = useRef(null)
  const editorRef = useRef(null)
  const onChangeRef = useRef(onChange)
  const valueRef = useRef(value)
  onChangeRef.current = onChange
  valueRef.current = value

  useEffect(() => {
    ensureMonacoYaml()
    const el = hostRef.current
    if (!el) return

    const suffix = SCHEMA_SUFFIX[schema] || SCHEMA_SUFFIX.machine
    const uri = monaco.Uri.parse(`file:///${encodeURIComponent(path)}.${suffix}`)
    const initial = valueRef.current || ''
    let model = monaco.editor.getModel(uri)
    if (model) {
      if (model.getValue() !== initial) model.setValue(initial)
    } else {
      model = monaco.editor.createModel(initial, 'yaml', uri)
    }

    const theme = monacoThemeFromDom()
    monaco.editor.setTheme(theme)
    const editor = monaco.editor.create(el, {
      model,
      theme,
      automaticLayout: true,
      readOnly,
      minimap: { enabled: false },
      fontSize: 13,
      fontFamily: 'JetBrains Mono, SFMono-Regular, Menlo, Monaco, Consolas, monospace',
      scrollBeyondLastLine: false,
      tabSize: 2,
      insertSpaces: true,
      wordWrap: 'on',
      padding: { top: 8, bottom: 8 },
      quickSuggestions: { other: true, comments: false, strings: true },
      renderLineHighlight: readOnly ? 'none' : 'line',
      overviewRulerLanes: 0,
      folding: true,
      glyphMargin: false,
      lineNumbersMinChars: 3,
      renderWhitespace: 'none',
      contextmenu: true,
      domReadOnly: readOnly,
    })
    editorRef.current = editor

    const sub = editor.onDidChangeModelContent(() => {
      onChangeRef.current?.(editor.getValue())
    })

    const obs = new MutationObserver(() => {
      monaco.editor.setTheme(monacoThemeFromDom())
    })
    obs.observe(document.documentElement, { attributes: true, attributeFilter: ['data-theme'] })

    return () => {
      sub.dispose()
      obs.disconnect()
      editor.dispose()
      editorRef.current = null
      model.dispose()
    }
  }, [path, schema, readOnly])

  useEffect(() => {
    const editor = editorRef.current
    if (!editor) return
    if ((value || '') !== editor.getValue()) {
      editor.setValue(value || '')
    }
  }, [value])

  return <div ref={hostRef} className={`yaml-editor ${className}`.trim()} />
}
