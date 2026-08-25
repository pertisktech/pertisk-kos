const CLUSTER_DEFAULTS = {
  network_mode: 'ipv4',
  cni: 'cilium',
  cp_vmid: 210,
  cp_memory: 4096,
  cp_cores: 2,
  cp_disk_gb: 50,
  worker_memory: 8192,
  worker_cores: 4,
  worker_disk_gb: 75,
  max_pods: 250,
  pod_subnet: '10.244.0.0/16',
  service_subnet: '10.96.0.0/12',
}

const ADDON_SECRET_FIELDS = {
  'cert-manager': [{ field: 'api_token', flag: 'token_set' }],
  ingress: [
    { field: 'admin_password', flag: 'token_set' },
    { field: 'registry_password', flag: 'registry_set' },
  ],
  'kos-scaler': [{ field: 'password', flag: 'token_set' }],
}

export function tfResourceName(raw, fallback = 'cluster') {
  let s = String(raw || fallback)
    .toLowerCase()
    .replace(/[^a-z0-9_]+/g, '_')
    .replace(/_+/g, '_')
    .replace(/^_|_$/g, '')
  if (!s) s = fallback
  if (!/^[a-z_]/.test(s)) s = `c_${s}`
  return s
}

export function tfQuote(value) {
  return `"${String(value).replace(/\\/g, '\\\\').replace(/"/g, '\\"').replace(/\n/g, '\\n')}"`
}

function attr(key, value, pad = 2) {
  const sp = ' '.repeat(pad)
  return `${sp}${key.padEnd(14)} = ${value}`
}

function mapBlock(name, entries, pad = 2) {
  if (!entries.length) return ''
  const sp = ' '.repeat(pad)
  const inner = entries.map(([k, v]) => `${sp}  ${k} = ${tfQuote(v)}`).join('\n')
  return `${sp}${name} = {\n${inner}\n${sp}}`
}

function str(v) {
  if (v == null) return ''
  return String(v).trim()
}

function addonSecrets(addon) {
  return ADDON_SECRET_FIELDS[addon.id] || []
}

function configEntries(addon) {
  const cfg = addon.config && typeof addon.config === 'object' ? addon.config : {}
  const secretNames = new Set(addonSecrets(addon).map((s) => s.field))
  const out = []
  for (const [key, raw] of Object.entries(cfg)) {
    if (secretNames.has(key)) continue
    if (raw == null) continue
    const value = typeof raw === 'object' ? JSON.stringify(raw) : String(raw)
    if (!value.trim()) continue
    out.push([key, value])
  }
  return out
}

function secretVarName(clusterName, addonId, field) {
  return tfResourceName(`${clusterName}_${addonId}_${field}`, 'secret')
}

function needsSecretVar(addon, spec) {
  if (spec.flag && addon[spec.flag]) return true
  if (addon.id === 'kos-scaler' && spec.field === 'password') return true
  return false
}

/**
 * @param {{ cluster: object, addons?: object[], mgmtUrl?: string, insecure?: boolean }} input
 */
export function generateClusterTerraform({ cluster, addons = [], mgmtUrl, insecure = true }) {
  const c = cluster || {}
  const name = tfResourceName(c.name)
  const providerName = tfResourceName(c.provider_name || 'hypervisor', 'hypervisor')
  const url = str(mgmtUrl) || 'https://pertisk-mgmt.example'
  const installed = (Array.isArray(addons) ? addons : []).filter(
    (a) => a && (a.status === 'installed' || a.status === 'partial' || a.status === 'error'),
  )

  const vars = [
    { name: 'pertisk_username', sensitive: false, comment: 'Local mgmt username' },
    { name: 'pertisk_password', sensitive: true, comment: 'Local mgmt password' },
  ]
  const seenVars = new Set(vars.map((v) => v.name))
  for (const addon of installed) {
    for (const spec of addonSecrets(addon)) {
      if (!needsSecretVar(addon, spec)) continue
      const n = secretVarName(c.name, addon.id, spec.field)
      if (seenVars.has(n)) continue
      seenVars.add(n)
      vars.push({
        name: n,
        sensitive: true,
        comment: `${addon.name || addon.id} ${spec.field}`,
      })
    }
  }

  const lines = []
  lines.push(`# Terraform for cluster ${c.name || name}`)
  lines.push(`# Cluster id: ${c.id || ''}`)
  lines.push('#')
  lines.push('# Bring this existing cluster into state (Terraform 1.5+):')
  lines.push('#   terraform init && terraform plan')
  lines.push('# Import blocks at the bottom bind resources to current mgmt objects.')
  lines.push('# Applying without import would create a new cluster with the same size.')
  lines.push('# Fill in secrets via TF_VAR_* or a tfvars file — they are never exported.')
  lines.push('')
  lines.push('terraform {')
  lines.push('  required_providers {')
  lines.push('    pertisk = {')
  lines.push('      source  = "pertisk-tech/pertisk"')
  lines.push('      version = "~> 0.1"')
  lines.push('    }')
  lines.push('  }')
  lines.push('}')
  lines.push('')

  for (const v of vars) {
    lines.push(`variable ${tfQuote(v.name)} {`)
    lines.push('  type        = string')
    if (v.sensitive) lines.push('  sensitive   = true')
    if (v.comment) lines.push(`  description = ${tfQuote(v.comment)}`)
    lines.push('}')
    lines.push('')
  }

  lines.push('provider "pertisk" {')
  lines.push(attr('url', tfQuote(url)))
  lines.push(attr('username', 'var.pertisk_username'))
  lines.push(attr('password', 'var.pertisk_password'))
  if (insecure) lines.push(attr('insecure', 'true'))
  lines.push('}')
  lines.push('')

  const providerLookup = str(c.provider_name)
    ? `  name = ${tfQuote(c.provider_name)}`
    : `  id   = ${tfQuote(c.provider_id || '')}`
  lines.push(`data "pertisk_provider" "${providerName}" {`)
  lines.push(providerLookup)
  lines.push('}')
  lines.push('')

  lines.push(`resource "pertisk_cluster" "${name}" {`)
  lines.push(attr('name', tfQuote(c.name || '')))
  lines.push(attr('provider_id', `data.pertisk_provider.${providerName}.id`))
  lines.push(attr('controlplanes', String(c.controlplanes ?? 1)))
  lines.push(attr('workers', String(c.workers ?? 1)))
  if (str(c.network_mode) && c.network_mode !== CLUSTER_DEFAULTS.network_mode) {
    lines.push(attr('network_mode', tfQuote(c.network_mode)))
  }
  if (str(c.vip)) lines.push(attr('vip', tfQuote(c.vip)))
  if (str(c.vip6)) lines.push(attr('vip6', tfQuote(c.vip6)))
  if (str(c.cni) && c.cni !== CLUSTER_DEFAULTS.cni) lines.push(attr('cni', tfQuote(c.cni)))
  if (str(c.k8s_version)) lines.push(attr('k8s_version', tfQuote(c.k8s_version)))
  if (c.cp_vmid != null && Number(c.cp_vmid) !== CLUSTER_DEFAULTS.cp_vmid) {
    lines.push(attr('cp_vmid', String(c.cp_vmid)))
  }
  if (c.cp_memory != null && Number(c.cp_memory) !== CLUSTER_DEFAULTS.cp_memory) {
    lines.push(attr('cp_memory', String(c.cp_memory)))
  }
  if (c.cp_cores != null && Number(c.cp_cores) !== CLUSTER_DEFAULTS.cp_cores) {
    lines.push(attr('cp_cores', String(c.cp_cores)))
  }
  if (c.cp_disk_gb != null && Number(c.cp_disk_gb) !== CLUSTER_DEFAULTS.cp_disk_gb) {
    lines.push(attr('cp_disk_gb', String(c.cp_disk_gb)))
  }
  if (c.worker_memory != null && Number(c.worker_memory) !== CLUSTER_DEFAULTS.worker_memory) {
    lines.push(attr('worker_memory', String(c.worker_memory)))
  }
  if (c.worker_cores != null && Number(c.worker_cores) !== CLUSTER_DEFAULTS.worker_cores) {
    lines.push(attr('worker_cores', String(c.worker_cores)))
  }
  if (c.worker_disk_gb != null && Number(c.worker_disk_gb) !== CLUSTER_DEFAULTS.worker_disk_gb) {
    lines.push(attr('worker_disk_gb', String(c.worker_disk_gb)))
  }
  if (c.max_pods != null && Number(c.max_pods) !== CLUSTER_DEFAULTS.max_pods) {
    lines.push(attr('max_pods', String(c.max_pods)))
  }
  if (str(c.arch)) lines.push(attr('arch', tfQuote(c.arch)))
  if (str(c.pod_subnet) && c.pod_subnet !== CLUSTER_DEFAULTS.pod_subnet) {
    lines.push(attr('pod_subnet', tfQuote(c.pod_subnet)))
  }
  if (str(c.service_subnet) && c.service_subnet !== CLUSTER_DEFAULTS.service_subnet) {
    lines.push(attr('service_subnet', tfQuote(c.service_subnet)))
  }
  if (str(c.pod_subnet_ipv6)) lines.push(attr('pod_subnet_ipv6', tfQuote(c.pod_subnet_ipv6)))
  if (str(c.service_subnet_ipv6)) lines.push(attr('service_subnet_ipv6', tfQuote(c.service_subnet_ipv6)))
  lines.push('}')
  lines.push('')

  const addonNames = []
  for (const addon of installed) {
    const res = tfResourceName(addon.id, 'addon')
    addonNames.push({ res, addon })
    lines.push(`resource "pertisk_addon" "${res}" {`)
    lines.push(attr('cluster_id', `pertisk_cluster.${name}.id`))
    lines.push(attr('addon', tfQuote(addon.id)))
    const cfg = mapBlock('config', configEntries(addon))
    if (cfg) lines.push(cfg)
    const secretEntries = addonSecrets(addon)
      .filter((spec) => needsSecretVar(addon, spec))
      .map((spec) => [spec.field, `var.${secretVarName(c.name, addon.id, spec.field)}`])
    if (secretEntries.length) {
      lines.push('  secrets = {')
      for (const [field, ref] of secretEntries) {
        lines.push(`    ${field} = ${ref}`)
      }
      lines.push('  }')
    }
    lines.push('}')
    lines.push('')
  }

  lines.push('# --- import existing objects (remove after first successful apply) ---')
  lines.push('import {')
  lines.push(`  to = pertisk_cluster.${name}`)
  lines.push(`  id = ${tfQuote(c.id || '')}`)
  lines.push('}')
  for (const { res, addon } of addonNames) {
    lines.push('')
    lines.push('import {')
    lines.push(`  to = pertisk_addon.${res}`)
    lines.push(`  id = ${tfQuote(`${c.id}/${addon.id}`)}`)
    lines.push('}')
  }
  lines.push('')

  return lines.join('\n')
}

export function terraformFilename(clusterName) {
  return `${tfResourceName(clusterName)}.tf`
}
