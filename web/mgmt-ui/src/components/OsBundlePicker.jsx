import { useCallback, useId, useRef, useState } from 'react'
import { Icon } from './Icons'

const SLOTS = [
  { key: 'kernel', label: 'kernel', hint: 'kernel or bzImage' },
  { key: 'initramfs', label: 'initramfs', hint: 'initramfs*.cpio.gz' },
  { key: 'manifest', label: 'manifest.json', hint: 'signed manifest' },
  { key: 'sig', label: 'manifest.sig', hint: 'Ed25519 signature' },
]

export function classifyOsBundleFiles(list) {
  const files = Array.from(list || [])
  const zip = files.find((f) => /\.zip$/i.test(f.name))
  if (zip) {
    return { zip, kernel: null, initramfs: null, manifest: null, sig: null }
  }
  const pick = (re) => files.find((f) => re.test(f.name))
  return {
    zip: null,
    kernel: pick(/^(kernel|bzImage)$/i) || pick(/bzImage/i),
    initramfs: pick(/^initramfs(\b|[.-])/i) || pick(/initramfs/i),
    manifest: pick(/^manifest\.json$/i),
    sig: pick(/^manifest\.sig$/i),
  }
}

export function mergeOsBundle(prev, list) {
  const incoming = classifyOsBundleFiles(list)
  if (incoming.zip) return incoming
  return {
    zip: null,
    kernel: incoming.kernel || prev?.kernel || null,
    initramfs: incoming.initramfs || prev?.initramfs || null,
    manifest: incoming.manifest || prev?.manifest || null,
    sig: incoming.sig || prev?.sig || null,
  }
}

export function osBundleReady(picked) {
  if (!picked) return false
  if (picked.zip) return true
  return !!(picked.kernel && picked.initramfs && picked.manifest && picked.sig)
}

function formatBytes(n) {
  if (n == null || Number.isNaN(n)) return ''
  if (n < 1024) return `${n} B`
  if (n < 1024 * 1024) return `${(n / 1024).toFixed(n < 10 * 1024 ? 1 : 0)} KB`
  return `${(n / (1024 * 1024)).toFixed(n < 10 * 1024 * 1024 ? 1 : 0)} MB`
}

function slotFile(bundle, key) {
  if (!bundle) return null
  if (bundle.zip) return key === 'zip' ? bundle.zip : null
  return bundle[key] || null
}

export default function OsBundlePicker({ value, onChange, disabled = false }) {
  const inputId = useId()
  const inputRef = useRef(null)
  const [dragOver, setDragOver] = useState(false)
  const ready = osBundleReady(value)
  const zip = value?.zip || null

  const applyFiles = useCallback(
    (list) => {
      if (!list?.length) return
      onChange(mergeOsBundle(value, list))
    },
    [onChange, value],
  )

  function onInputChange(e) {
    applyFiles(e.target.files)
    e.target.value = ''
  }

  function onDrop(e) {
    e.preventDefault()
    setDragOver(false)
    if (disabled) return
    applyFiles(e.dataTransfer.files)
  }

  function clearAll(e) {
    e.stopPropagation()
    onChange(null)
  }

  function removeSlot(e, key) {
    e.stopPropagation()
    if (key === 'zip' || value?.zip) {
      onChange(null)
      return
    }
    onChange({ ...value, [key]: null, zip: null })
  }

  const title = zip
    ? zip.name
    : ready
      ? 'Signed bundle ready'
      : 'Drop files here, or choose'

  return (
    <div className={`os-picker ${ready ? 'is-ready' : ''} ${dragOver ? 'is-drag' : ''} ${disabled ? 'is-disabled' : ''}`}>
      <input
        id={inputId}
        ref={inputRef}
        type="file"
        multiple
        accept=".zip,.json,.sig,.gz,application/zip"
        className="os-picker-input"
        disabled={disabled}
        onChange={onInputChange}
      />
      <div
        className="os-picker-drop"
        role="button"
        tabIndex={disabled ? -1 : 0}
        aria-controls={inputId}
        aria-disabled={disabled}
        onClick={() => !disabled && inputRef.current?.click()}
        onKeyDown={(e) => {
          if (disabled) return
          if (e.key === 'Enter' || e.key === ' ') {
            e.preventDefault()
            inputRef.current?.click()
          }
        }}
        onDragEnter={(e) => {
          e.preventDefault()
          if (!disabled) setDragOver(true)
        }}
        onDragOver={(e) => {
          e.preventDefault()
          e.dataTransfer.dropEffect = 'copy'
        }}
        onDragLeave={(e) => {
          if (!e.currentTarget.contains(e.relatedTarget)) setDragOver(false)
        }}
        onDrop={onDrop}
      >
        <span className={`os-picker-icon ${ready ? 'ok' : ''}`}>
          <Icon name={ready ? 'check' : 'upload'} size={22} />
        </span>
        <div className="os-picker-copy">
          <strong>{title}</strong>
          <span className="muted">
            {zip
              ? formatBytes(zip.size)
              : 'kernel · initramfs · manifest.json · manifest.sig · os-trust.pk  ·  or one .zip'}
          </span>
        </div>
        <span className="os-picker-choose">Choose files</span>
      </div>

      {zip ? (
        <ul className="os-picker-files">
          <li className="os-picker-file is-on">
            <Icon name="check" size={14} />
            <span className="os-picker-file-name">{zip.name}</span>
            <span className="muted">{formatBytes(zip.size)}</span>
            <button
              type="button"
              className="os-picker-x"
              onClick={(e) => removeSlot(e, 'zip')}
              disabled={disabled}
              aria-label="Remove zip"
            >
              <Icon name="x" size={14} />
            </button>
          </li>
        </ul>
      ) : (
        <ul className="os-picker-files">
          {SLOTS.map((slot) => {
            const file = slotFile(value, slot.key)
            return (
              <li key={slot.key} className={`os-picker-file ${file ? 'is-on' : ''}`}>
                <Icon name={file ? 'check' : 'plus'} size={14} />
                <span className="os-picker-file-name">{file ? file.name : slot.label}</span>
                <span className="muted">{file ? formatBytes(file.size) : slot.hint}</span>
                {file ? (
                  <button
                    type="button"
                    className="os-picker-x"
                    onClick={(e) => removeSlot(e, slot.key)}
                    disabled={disabled}
                    aria-label={`Remove ${slot.label}`}
                  >
                    <Icon name="x" size={14} />
                  </button>
                ) : null}
              </li>
            )
          })}
        </ul>
      )}

      {value && (zip || value.kernel || value.initramfs || value.manifest || value.sig) ? (
        <button type="button" className="os-picker-clear" onClick={clearAll} disabled={disabled}>
          Clear
        </button>
      ) : null}
    </div>
  )
}
