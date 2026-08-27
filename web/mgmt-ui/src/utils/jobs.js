import { formatDuration, parseDate } from './datetime'

const KIND_LABELS = {
  create_cluster: 'Create cluster',
  delete_cluster: 'Delete cluster',
  add_node: 'Add node',
  adopt_node: 'Adopt node',
  remove_node: 'Remove node',
  resize_node: 'Resize node',
  reboot_node: 'Reboot node',
  upgrade_cluster: 'Upgrade Kubernetes',
  upgrade_os: 'Upgrade OS',
  update_config: 'Update config',
  install_addon: 'Install add-on',
}

export function jobKindLabel(kind) {
  if (!kind) return '—'
  return KIND_LABELS[kind] || kind.replace(/_/g, ' ')
}

export function jobStatusLabel(status) {
  if (!status) return '—'
  return status.charAt(0).toUpperCase() + status.slice(1)
}

export function jobIsLive(job) {
  return job?.status === 'running' || job?.status === 'queued'
}

export function jobDurationLabel(job, now = Date.now()) {
  const start = parseDate(job?.created_at)
  if (!start) return '—'
  const end = jobIsLive(job)
    ? new Date(now)
    : parseDate(job.finished_at) || parseDate(job.updated_at)
  if (!end) return '—'
  return formatDuration(Math.max(0, end.getTime() - start.getTime()))
}
