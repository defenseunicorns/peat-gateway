import type {
  CdcSinkConfig,
  EnrollmentAuditEntry,
  EnrollmentToken,
  FormationConfig,
  HealthResponse,
  IdpConfig,
  Organization,
  PolicyRule,
} from './types'

export class ApiError extends Error {
  constructor(
    public status: number,
    message: string,
  ) {
    super(message)
    this.name = 'ApiError'
  }
}

async function request<T>(path: string, init?: RequestInit): Promise<T> {
  const res = await fetch(path, init)
  if (!res.ok) {
    const text = await res.text().catch(() => res.statusText)
    throw new ApiError(res.status, text)
  }
  return res.json()
}

export const api = {
  // Health
  health: () => request<HealthResponse>('/health'),

  // Organizations
  listOrgs: () => request<Organization[]>('/orgs'),
  getOrg: (orgId: string) => request<Organization>(`/orgs/${orgId}`),

  // Formations
  listFormations: (orgId: string) => request<FormationConfig[]>(`/orgs/${orgId}/formations`),
  getFormation: (orgId: string, appId: string) =>
    request<FormationConfig>(`/orgs/${orgId}/formations/${appId}`),

  // Enrollment Tokens
  listTokens: (orgId: string, appId: string) =>
    request<EnrollmentToken[]>(`/orgs/${orgId}/formations/${appId}/tokens`),
  getToken: (orgId: string, tokenId: string) =>
    request<EnrollmentToken>(`/orgs/${orgId}/tokens/${tokenId}`),

  // CDC Sinks
  listSinks: (orgId: string) => request<CdcSinkConfig[]>(`/orgs/${orgId}/sinks`),
  getSink: (orgId: string, sinkId: string) =>
    request<CdcSinkConfig>(`/orgs/${orgId}/sinks/${sinkId}`),

  // Identity Providers
  listIdps: (orgId: string) => request<IdpConfig[]>(`/orgs/${orgId}/idps`),
  getIdp: (orgId: string, idpId: string) => request<IdpConfig>(`/orgs/${orgId}/idps/${idpId}`),

  // Policy Rules
  listPolicyRules: (orgId: string) => request<PolicyRule[]>(`/orgs/${orgId}/policy-rules`),

  // Audit
  listAudit: (orgId: string, appId?: string, limit?: number) => {
    const params = new URLSearchParams()
    if (appId) params.set('app_id', appId)
    if (limit) params.set('limit', limit.toString())
    const qs = params.toString()
    return request<EnrollmentAuditEntry[]>(`/orgs/${orgId}/audit${qs ? `?${qs}` : ''}`)
  },

  // Formation detail (stubs)
  listPeers: (orgId: string, appId: string) =>
    request<unknown[]>(`/orgs/${orgId}/formations/${appId}/peers`),
  listDocuments: (orgId: string, appId: string) =>
    request<unknown[]>(`/orgs/${orgId}/formations/${appId}/documents`),
  listCertificates: (orgId: string, appId: string) =>
    request<unknown[]>(`/orgs/${orgId}/formations/${appId}/certificates`),
}
