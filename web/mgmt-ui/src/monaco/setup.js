import * as monaco from 'monaco-editor'
import { configureMonacoYaml } from 'monaco-yaml'
import EditorWorker from 'monaco-editor/esm/vs/editor/editor.worker?worker'
import YamlWorker from './yaml.worker.js?worker'
import { machineConfigSchema } from './schemas/machineConfig'
import { kubeconfigSchema } from './schemas/kubeconfig'
import 'monaco-editor/min/vs/editor/editor.main.css'

export { monaco }

let configured = false

export function monacoThemeFromDom() {
  return document.documentElement.getAttribute('data-theme') === 'light'
    ? 'pertisk-light'
    : 'pertisk-dark'
}

/** Themes aligned with Vela / --app-* tokens in index.css. */
function definePertiskThemes() {
  monaco.editor.defineTheme('pertisk-dark', {
    base: 'vs-dark',
    inherit: true,
    rules: [],
    colors: {
      'editor.background': '#0c0e15',
      'editorGutter.background': '#0c0e15',
      'minimap.background': '#0c0e15',
      'editor.lineHighlightBackground': '#1a1d2866',
      'editorLineNumber.foreground': '#5f6678',
      'editorLineNumber.activeForeground': '#f3f4f9',
      'editorCursor.foreground': '#7c5cff',
      'editor.selectionBackground': '#7c5cff40',
      'editor.inactiveSelectionBackground': '#7c5cff22',
      'editorWidget.background': '#12141d',
      'editorWidget.border': 'rgba(255,255,255,0.07)',
      'editorSuggestWidget.background': '#12141d',
      'editorSuggestWidget.border': 'rgba(255,255,255,0.07)',
      'editorHoverWidget.background': '#12141d',
      'editorHoverWidget.border': 'rgba(255,255,255,0.07)',
      'input.background': '#12141d',
      'focusBorder': '#7c5cff',
    },
  })
  monaco.editor.defineTheme('pertisk-light', {
    base: 'vs',
    inherit: true,
    rules: [],
    colors: {
      'editor.background': '#f5f6fb',
      'editorGutter.background': '#f5f6fb',
      'minimap.background': '#f5f6fb',
      'editor.lineHighlightBackground': '#f3f4f966',
      'editorLineNumber.foreground': '#9097a8',
      'editorLineNumber.activeForeground': '#171a23',
      'editorCursor.foreground': '#6a4cff',
      'editor.selectionBackground': '#6a4cff40',
      'editor.inactiveSelectionBackground': '#6a4cff22',
      'editorWidget.background': '#ffffff',
      'editorWidget.border': 'rgba(17,20,32,0.08)',
      'editorSuggestWidget.background': '#ffffff',
      'editorSuggestWidget.border': 'rgba(17,20,32,0.08)',
      'editorHoverWidget.background': '#ffffff',
      'editorHoverWidget.border': 'rgba(17,20,32,0.08)',
      'input.background': '#ffffff',
      'focusBorder': '#6a4cff',
    },
  })
}

/** Configure monaco-yaml once (only one instance is allowed). */
export function ensureMonacoYaml() {
  if (configured) return
  configured = true

  definePertiskThemes()

  globalThis.MonacoEnvironment = {
    getWorker(_moduleId, label) {
      switch (label) {
        case 'editorWorkerService':
          return new EditorWorker()
        case 'yaml':
          return new YamlWorker()
        default:
          throw new Error(`Unknown Monaco worker ${label}`)
      }
    },
  }

  configureMonacoYaml(monaco, {
    enableSchemaRequest: false,
    hover: true,
    completion: true,
    validate: true,
    format: true,
    schemas: [
      {
        uri: 'inmemory://schema/pertisk-machine-config.json',
        fileMatch: ['**/*.machine.yaml', '**/machine.yaml'],
        schema: machineConfigSchema,
      },
      {
        uri: 'inmemory://schema/kubeconfig.json',
        fileMatch: ['**/*.kubeconfig.yaml', '**/kubeconfig.yaml'],
        schema: kubeconfigSchema,
      },
    ],
  })
}
