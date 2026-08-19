import { useCallback, useEffect, useState } from 'react'
import { api } from '../api'
import { Icon } from '../components/Icons'
import Modal from '../components/Modal'
import { useConfirm } from '../components/Confirm'
import { formatArch } from '../components/ClusterMetaBadges'

function formatBytes(n) {
  const v = Number(n) || 0
  if (v < 1024) return `${v} B`
  if (v < 1024 * 1024) return `${(v / 1024).toFixed(v < 10 * 1024 ? 1 : 0)} KB`
  return `${(v / (1024 * 1024)).toFixed(v < 10 * 1024 * 1024 ? 1 : 0)} MB`
}

export default function Images() {
  const confirm = useConfirm()
  const [catalog, setCatalog] = useState({ dir: '', images: [], ready: { amd64: false, arm64: false } })
  const [error, setError] = useState('')
  const [busy, setBusy] = useState(false)
  const [uploadOpen, setUploadOpen] = useState(false)
  const [arch, setArch] = useState('amd64')
  const [file, setFile] = useState(null)

  const load = useCallback(() => {
    api('/images')
      .then((c) => {
        setCatalog(c && typeof c === 'object' ? c : { dir: '', images: [], ready: {} })
        setError('')
      })
      .catch((e) => setError(e.message || 'failed to load images'))
  }, [])

  useEffect(() => {
    load()
  }, [load])

  async function upload() {
    if (!file) return
    setBusy(true)
    setError('')
    try {
      const fd = new FormData()
      fd.append('arch', arch)
      fd.append('image', file)
      await api('/images', { method: 'POST', body: fd })
      setUploadOpen(false)
      setFile(null)
      load()
    } catch (e) {
      setError(e.message || 'upload failed')
    } finally {
      setBusy(false)
    }
  }

  async function remove(img) {
    const ok = await confirm({
      title: 'Delete cloud image',
      message: `Remove ${img.name} from the mgmt host? Existing clusters keep their disks.`,
      confirmLabel: 'Delete',
      tone: 'danger',
    })
    if (!ok) return
    try {
      await api(`/images/${encodeURIComponent(img.name)}`, { method: 'DELETE' })
      load()
    } catch (e) {
      setError(e.message || 'delete failed')
    }
  }

  const list = Array.isArray(catalog.images) ? catalog.images : []
  const readyAmd = !!catalog.ready?.amd64
  const readyArm = !!catalog.ready?.arm64

  return (
    <div>
      <div className="page-head">
        <h1>
          <Icon name="disk" size={22} /> Images
        </h1>
        <button type="button" className="btn btn-icon" onClick={() => setUploadOpen(true)}>
          <Icon name="upload" size={16} /> Upload qcow2
        </button>
      </div>
      <p className="muted" style={{ marginTop: '-0.35rem', marginBottom: '1rem' }}>
        Guest install disks for cluster create. Download
        {' '}
        <span className="mono-inline">pertisk-cloud-*-v*.qcow2</span>
        {' '}
        from the GitHub Release, or build with
        {' '}
        <span className="mono-inline">make cloud ARCH=amd64</span>
        {' / '}
        <span className="mono-inline">ARCH=arm64</span>
        . Mgmt does not compile the OS.
      </p>
      {error && <div className="error">{error}</div>}
      <div className="row-actions" style={{ marginBottom: '0.75rem', gap: '0.5rem' }}>
        <span className={`badge ${readyAmd ? 'ready' : ''}`}>amd64 {readyAmd ? 'ready' : 'missing'}</span>
        <span className={`badge ${readyArm ? 'ready' : ''}`}>arm64 {readyArm ? 'ready' : 'missing'}</span>
        {catalog.dir && (
          <span className="muted">
            <span className="mono-inline">{catalog.dir}</span>
          </span>
        )}
      </div>
      <div className="card">
        <table>
          <thead>
            <tr>
              <th>File</th>
              <th>Arch</th>
              <th>Role</th>
              <th>Size</th>
              <th />
            </tr>
          </thead>
          <tbody>
            {list.map((img) => (
              <tr key={img.name}>
                <td>
                  <span className="mono-inline">{img.name}</span>
                  {img.is_default && (
                    <span className="badge ready" style={{ marginLeft: 8 }}>
                      default
                    </span>
                  )}
                </td>
                <td>
                  <span className={`badge arch arch-${formatArch(img.arch)}`}>
                    {formatArch(img.arch)}
                  </span>
                </td>
                <td className="muted">{img.role || '—'}</td>
                <td className="muted">{formatBytes(img.size_bytes)}</td>
                <td>
                  <div style={{ display: 'flex', gap: '0.35rem', justifyContent: 'flex-end' }}>
                    <button
                      type="button"
                      className="secondary btn-icon"
                      onClick={() => remove(img)}
                      title="Delete"
                    >
                      <Icon name="trash" size={14} />
                    </button>
                  </div>
                </td>
              </tr>
            ))}
          </tbody>
        </table>
        {list.length === 0 && (
          <p className="muted">
            No qcow2 yet. Upload <span className="mono-inline">pertisk-cloud-amd64*.qcow2</span> and/or{' '}
            <span className="mono-inline">pertisk-cloud-arm64*.qcow2</span> from the GitHub Release
            before Create Cluster.
          </p>
        )}
      </div>

      <Modal
        open={uploadOpen}
        title="Upload cloud image"
        icon="upload"
        onClose={() => !busy && setUploadOpen(false)}
      >
        <p className="muted">
          Prefer files named <span className="mono-inline">pertisk-cloud-amd64.qcow2</span> or{' '}
          <span className="mono-inline">pertisk-cloud-arm64.qcow2</span>. Other names are stored as
          the default disk for the arch you pick. Same name replaces the previous file.
        </p>
        <div className="field">
          <label>Arch</label>
          <select
            value={arch}
            onChange={(e) => setArch(e.target.value)}
            disabled={busy}
          >
            <option value="amd64">amd64</option>
            <option value="arm64">arm64</option>
          </select>
        </div>
        <div className="field">
          <label>qcow2</label>
          <input
            type="file"
            accept=".qcow2,application/octet-stream"
            disabled={busy}
            onChange={(e) => {
              const f = e.target.files?.[0] || null
              setFile(f)
              const n = f?.name || ''
              if (/arm64|aarch64/i.test(n)) setArch('arm64')
              else if (/amd64|x86_64/i.test(n)) setArch('amd64')
            }}
          />
          {file && <p className="hint muted">{file.name}</p>}
        </div>
        <div className="form-footer">
          <button type="button" className="secondary" onClick={() => setUploadOpen(false)} disabled={busy}>
            Cancel
          </button>
          <button type="button" className="btn-icon" disabled={busy || !file} onClick={upload}>
            <Icon name="check" size={16} /> {busy ? 'Uploading…' : 'Save image'}
          </button>
        </div>
      </Modal>
    </div>
  )
}
