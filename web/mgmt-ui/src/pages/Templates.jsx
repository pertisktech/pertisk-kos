import { lazy, Suspense, useCallback, useEffect, useState } from 'react'
import { api } from '../api'
import { Icon } from '../components/Icons'
import PageHeader from '../components/PageHeader'
import Modal from '../components/Modal'
import { useConfirm } from '../components/Confirm'
import { defaultTemplateYaml } from '../utils/machineConfig'

const YamlEditor = lazy(() => import('../components/YamlEditor'))

const DEFAULT_FORM = {
  name: '',
  description: '',
  role: 'any',
  yaml: defaultTemplateYaml(''),
}

export default function Templates() {
  const confirm = useConfirm()
  const [list, setList] = useState([])
  const [error, setError] = useState('')
  const [open, setOpen] = useState(false)
  const [editingId, setEditingId] = useState(null)
  const [publicUrl, setPublicUrl] = useState('')
  const [form, setForm] = useState(DEFAULT_FORM)
  const [busy, setBusy] = useState(false)

  const load = useCallback(() => {
    api('/templates')
      .then((rows) => setList(Array.isArray(rows) ? rows : []))
      .catch((e) => setError(e.message || 'failed to load templates'))
  }, [])

  useEffect(() => {
    load()
    api('/settings')
      .then((s) => setPublicUrl(String(s?.public_url || '').trim()))
      .catch(() => {})
  }, [load])

  function openCreate() {
    setEditingId(null)
    setForm({
      ...DEFAULT_FORM,
      yaml: defaultTemplateYaml(publicUrl),
    })
    setOpen(true)
  }

  function openEdit(t) {
    setEditingId(t.id)
    setForm({
      name: t.name,
      description: t.description || '',
      role: t.role || 'any',
      yaml: t.yaml || '',
    })
    setOpen(true)
  }

  async function save() {
    setBusy(true)
    setError('')
    try {
      const body = {
        name: form.name,
        description: form.description,
        role: form.role,
        yaml: form.yaml,
      }
      if (editingId) {
        await api(`/templates/${editingId}`, { method: 'PUT', body })
      } else {
        await api('/templates', { method: 'POST', body })
      }
      setOpen(false)
      load()
    } catch (e) {
      setError(e.message || 'save failed')
    } finally {
      setBusy(false)
    }
  }

  async function remove(t) {
    const ok = await confirm({
      title: 'Delete template',
      message: `Delete template “${t.name}”? This cannot be undone.`,
      confirmLabel: 'Delete',
      tone: 'danger',
    })
    if (!ok) return
    try {
      await api(`/templates/${t.id}`, { method: 'DELETE' })
      load()
    } catch (e) {
      setError(e.message || 'delete failed')
    }
  }

  return (
    <div className="dash-page">
      <PageHeader
        title="Templates"
        description="Reusable node blueprints that pin an image, arch, and topology."
        actions={
          <button type="button" className="btn btn-icon" onClick={openCreate}>
            <Icon name="plus" size={16} /> New template
          </button>
        }
      />
      {error && <div className="error">{error}</div>}
      {list.length === 0 ? (
        <div className="card dash-empty">
          <p className="muted" style={{ margin: 0 }}>
            No templates yet. Create a machine-config blueprint to reuse on cluster Config tabs.
          </p>
        </div>
      ) : (
        <section className="entity-grid entity-grid-3">
          {list.map((t) => (
            <article key={t.id} className="entity-card">
              <div className="entity-card-head">
                <div className="entity-card-identity">
                  <span className="entity-card-icon" aria-hidden>
                    <Icon name="templates" size={18} />
                  </span>
                  <div className="entity-card-copy">
                    <p className="entity-card-name">{t.name}</p>
                    <p className="entity-card-sub">{t.role || 'any'}</p>
                  </div>
                </div>
              </div>
              <p className="entity-card-desc">{t.description || 'No description.'}</p>
              <div className="entity-card-foot">
                <div className="entity-card-tags">
                  <span className="tag tag-outline">{t.role || 'any'}</span>
                  <span className="entity-card-stamp">{t.updated_at || '—'}</span>
                </div>
                <div className="row-actions">
                  <button
                    type="button"
                    className="secondary btn-icon"
                    onClick={() => openEdit(t)}
                    title="Edit"
                  >
                    <Icon name="edit" size={14} />
                  </button>
                  <button
                    type="button"
                    className="danger btn-icon"
                    onClick={() => remove(t)}
                    title="Delete"
                  >
                    <Icon name="trash" size={14} />
                  </button>
                </div>
              </div>
            </article>
          ))}
        </section>
      )}

      <Modal
        open={open}
        wide
        cardClassName="modal-yaml"
        title={editingId ? 'Edit template' : 'New template'}
        icon="templates"
        onClose={() => setOpen(false)}
      >
        <div className="field">
          <label>Name</label>
          <input
            value={form.name}
            onChange={(e) => setForm({ ...form, name: e.target.value })}
            placeholder="worker-baseline"
          />
        </div>
        <div className="field">
          <label>Description</label>
          <input
            value={form.description}
            onChange={(e) => setForm({ ...form, description: e.target.value })}
            placeholder="Optional"
          />
        </div>
        <div className="field">
          <label>Role</label>
          <select
            value={form.role}
            onChange={(e) => setForm({ ...form, role: e.target.value })}
          >
            <option value="any">any</option>
            <option value="controlplane">controlplane</option>
            <option value="worker">worker</option>
          </select>
        </div>
        <div className="field">
          <label>YAML</label>
          <Suspense fallback={<div className="yaml-editor yaml-editor--modal muted">Loading editor…</div>}>
            <YamlEditor
              className="yaml-editor--modal"
              schema="machine"
              path={`template-${editingId || 'new'}`}
              value={form.yaml}
              onChange={(yaml) => setForm((f) => ({ ...f, yaml }))}
            />
          </Suspense>
        </div>
        <div className="form-footer">
          <button type="button" className="secondary" onClick={() => setOpen(false)}>
            Cancel
          </button>
          <button type="button" className="btn-icon" disabled={busy || !form.name.trim()} onClick={save}>
            <Icon name="check" size={16} /> {busy ? 'Saving…' : 'Save'}
          </button>
        </div>
      </Modal>
    </div>
  )
}
