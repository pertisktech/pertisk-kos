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

function definePertiskThemes() {
  monaco.editor.defineTheme('pertisk-dark', {
    base: 'vs-dark',
    inherit: true,
    rules: [],
    colors: {
      'editor.background': '#0c0d18',
      'editorGutter.background': '#0c0d18',
      'minimap.background': '#0c0d18',
      'editor.lineHighlightBackground': '#1d1f3266',
      'editorLineNumber.foreground': '#8e90ad',
      'editorLineNumber.activeForeground': '#e6e7f0',
      'editorCursor.foreground': '#9a7bf7',
      'editor.selectionBackground': '#9a7bf740',
      'editor.inactiveSelectionBackground': '#9a7bf722',
      'editorWidget.background': '#131421',
      'editorWidget.border': '#23253c',
      'editorSuggestWidget.background': '#131421',
      'editorSuggestWidget.border': '#23253c',
      'editorHoverWidget.background': '#131421',
      'editorHoverWidget.border': '#23253c',
      'input.background': '#131421',
      'focusBorder': '#9a7bf7',
    },
  })
  monaco.editor.defineTheme('pertisk-light', {
    base: 'vs',
    inherit: true,
    rules: [],
    colors: {
      'editor.background': '#f5f5fa',
      'editorGutter.background': '#f5f5fa',
      'minimap.background': '#f5f5fa',
      'editor.lineHighlightBackground': '#eeeef866',
      'editorLineNumber.foreground': '#6c6d90',
      'editorLineNumber.activeForeground': '#16162a',
      'editorCursor.foreground': '#6d3ef5',
      'editor.selectionBackground': '#6d3ef540',
      'editor.inactiveSelectionBackground': '#6d3ef522',
      'editorWidget.background': '#ffffff',
      'editorWidget.border': '#e2e2ec',
      'editorSuggestWidget.background': '#ffffff',
      'editorSuggestWidget.border': '#e2e2ec',
      'editorHoverWidget.background': '#ffffff',
      'editorHoverWidget.border': '#e2e2ec',
      'input.background': '#ffffff',
      'focusBorder': '#6d3ef5',
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
