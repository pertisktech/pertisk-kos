/** JSON Schema for a kubeconfig (apiVersion: v1, kind: Config). */
export const kubeconfigSchema = {
  $schema: 'http://json-schema.org/draft-07/schema#',
  $id: 'inmemory://schema/kubeconfig.json',
  title: 'Kubeconfig',
  description: 'Kubernetes client configuration for kubectl / kube-web.',
  type: 'object',
  required: ['apiVersion', 'kind'],
  additionalProperties: true,
  properties: {
    apiVersion: {
      type: 'string',
      const: 'v1',
      description: 'Kubeconfig API version.',
    },
    kind: {
      type: 'string',
      const: 'Config',
      description: 'Must be Config.',
    },
    'current-context': {
      type: 'string',
      description: 'Name of the context to use by default.',
    },
    preferences: {
      type: 'object',
      additionalProperties: true,
      description: 'Optional client preferences.',
    },
    clusters: {
      type: 'array',
      description: 'Named cluster entries (server + CA).',
      items: {
        type: 'object',
        additionalProperties: true,
        required: ['name', 'cluster'],
        properties: {
          name: { type: 'string', description: 'Cluster name referenced by contexts.' },
          cluster: {
            type: 'object',
            additionalProperties: true,
            properties: {
              server: { type: 'string', description: 'API server URL.' },
              'certificate-authority-data': {
                type: 'string',
                description: 'Base64-encoded cluster CA certificate.',
              },
              'certificate-authority': {
                type: 'string',
                description: 'Path to a CA certificate file.',
              },
              'insecure-skip-tls-verify': {
                type: 'boolean',
                description: 'Skip TLS verification (lab only).',
              },
              'tls-server-name': {
                type: 'string',
                description: 'Server name for TLS SNI / cert verification.',
              },
            },
          },
        },
      },
    },
    users: {
      type: 'array',
      description: 'Named user credentials.',
      items: {
        type: 'object',
        additionalProperties: true,
        required: ['name', 'user'],
        properties: {
          name: { type: 'string', description: 'User name referenced by contexts.' },
          user: {
            type: 'object',
            additionalProperties: true,
            properties: {
              'client-certificate-data': {
                type: 'string',
                description: 'Base64-encoded client certificate.',
              },
              'client-key-data': {
                type: 'string',
                description: 'Base64-encoded client private key.',
              },
              'client-certificate': { type: 'string', description: 'Path to a client certificate file.' },
              'client-key': { type: 'string', description: 'Path to a client key file.' },
              token: { type: 'string', description: 'Bearer token.' },
              username: { type: 'string' },
              password: { type: 'string' },
              exec: {
                type: 'object',
                additionalProperties: true,
                description: 'Exec plugin for credential helpers.',
                properties: {
                  apiVersion: { type: 'string' },
                  command: { type: 'string' },
                  args: { type: 'array', items: { type: 'string' } },
                  env: {
                    type: 'array',
                    items: {
                      type: 'object',
                      properties: {
                        name: { type: 'string' },
                        value: { type: 'string' },
                      },
                    },
                  },
                },
              },
            },
          },
        },
      },
    },
    contexts: {
      type: 'array',
      description: 'Named bindings of user + cluster (+ namespace).',
      items: {
        type: 'object',
        additionalProperties: true,
        required: ['name', 'context'],
        properties: {
          name: { type: 'string', description: 'Context name (matches current-context).' },
          context: {
            type: 'object',
            additionalProperties: true,
            properties: {
              cluster: { type: 'string', description: 'Cluster name from clusters[].name.' },
              user: { type: 'string', description: 'User name from users[].name.' },
              namespace: { type: 'string', description: 'Default namespace for kubectl.' },
            },
          },
        },
      },
    },
  },
}
