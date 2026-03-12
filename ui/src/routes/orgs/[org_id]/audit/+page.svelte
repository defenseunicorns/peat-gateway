<script lang="ts">
  import { page } from '$app/state'
  import Report from 'carbon-icons-svelte/lib/Report.svelte'

  import type { EnrollmentAuditEntry } from '$lib/api/types'
  import { api, ApiError } from '$lib/api/client'
  import Badge from '$components/Badge.svelte'
  import DataTable from '$components/DataTable.svelte'
  import EmptyState from '$components/EmptyState.svelte'

  let orgId = $derived(page.params.org_id)

  let entries: EnrollmentAuditEntry[] = $state([])
  let loading = $state(true)
  let error: string | null = $state(null)

  function decisionText(entry: EnrollmentAuditEntry): string {
    if ('Approved' in entry.decision) return 'Approved'
    if ('Denied' in entry.decision) return 'Denied'
    return 'Unknown'
  }

  function decisionTier(entry: EnrollmentAuditEntry): string {
    if ('Approved' in entry.decision) return entry.decision.Approved.tier
    return ''
  }

  function decisionReason(entry: EnrollmentAuditEntry): string {
    if ('Denied' in entry.decision) return entry.decision.Denied.reason
    return ''
  }

  function formatTimestamp(ms: number): string {
    return new Date(ms).toLocaleString('en-US', {
      year: 'numeric',
      month: 'short',
      day: 'numeric',
      hour: '2-digit',
      minute: '2-digit',
    })
  }

  async function load(id: string) {
    loading = true
    error = null
    try {
      entries = await api.listAudit(id, undefined, 100)
    } catch (e) {
      error = e instanceof ApiError ? e.message : 'Failed to load audit log'
    } finally {
      loading = false
    }
  }

  $effect(() => {
    if (orgId) load(orgId)
  })
</script>

<div>
  <div class="mb-6 flex items-center gap-3">
    <Report size={24} class="text-gray-400" />
    <h1 class="text-2xl font-semibold text-white">Enrollment Audit Log</h1>
    <span class="text-sm text-gray-500">({orgId})</span>
  </div>

  {#if loading}
    <div class="card animate-pulse">
      <div class="h-40 rounded bg-gray-800"></div>
    </div>
  {:else if error}
    <div class="rounded-lg bg-red-900/30 p-4 text-red-200">{error}</div>
  {:else if entries.length === 0}
    <EmptyState message="No audit entries" />
  {:else}
    <DataTable columns={['Subject', 'App', 'Decision', 'Tier', 'Timestamp']}>
      {#each entries as entry (entry.audit_id)}
        <tr class="table-row">
          <td class="data-cell font-medium text-white">{entry.subject}</td>
          <td class="data-cell font-mono text-xs">{entry.app_id}</td>
          <td class="data-cell">
            {#if decisionText(entry) === 'Approved'}
              <Badge variant="green">Approved</Badge>
            {:else}
              <Badge variant="red">Denied</Badge>
            {/if}
          </td>
          <td class="data-cell">{decisionTier(entry)}</td>
          <td class="data-cell text-gray-500">{formatTimestamp(entry.timestamp_ms)}</td>
        </tr>
      {/each}
    </DataTable>
  {/if}
</div>
