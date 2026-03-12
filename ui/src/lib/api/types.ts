export interface Organization {
  org_id: string
  display_name: string
  quotas: OrgQuotas
  created_at: number
}

export interface OrgQuotas {
  max_formations: number
  max_peers_per_formation: number
  max_documents_per_formation: number
  max_cdc_sinks: number
  max_enrollments_per_hour: number
}

export interface FormationConfig {
  app_id: string
  mesh_id: string
  enrollment_policy: EnrollmentPolicy
}

export type EnrollmentPolicy = 'Open' | 'Controlled' | 'Strict'

export interface EnrollmentToken {
  token_id: string
  org_id: string
  app_id: string
  label: string
  max_uses: number | null
  uses: number
  expires_at: number | null
  created_at: number
  revoked: boolean
}

export interface CdcSinkConfig {
  sink_id: string
  org_id: string
  sink_type: CdcSinkType
  enabled: boolean
  created_at: number
}

export type CdcSinkType =
  | { Nats: { subject_prefix: string } }
  | { Kafka: { topic: string } }
  | { Webhook: { url: string } }

export interface IdpConfig {
  idp_id: string
  org_id: string
  issuer_url: string
  client_id: string
  client_secret: string
  enabled: boolean
  created_at: number
}

export interface PolicyRule {
  rule_id: string
  org_id: string
  claim_key: string
  claim_value: string
  tier: MeshTier
  permissions: number
  priority: number
}

export type MeshTier = 'Authority' | 'Infrastructure' | 'Endpoint'

export interface EnrollmentAuditEntry {
  audit_id: string
  org_id: string
  app_id: string
  idp_id: string
  subject: string
  decision: EnrollmentDecision
  timestamp_ms: number
}

export type EnrollmentDecision =
  | { Approved: { tier: MeshTier; permissions: number } }
  | { Denied: { reason: string } }

export interface HealthResponse {
  status: string
  version: string
}
