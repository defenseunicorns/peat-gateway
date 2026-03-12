<script lang="ts">
  import { page } from '$app/state'
  import Password from 'carbon-icons-svelte/lib/Password.svelte'

  import type { EnrollmentToken, FormationConfig } from '$lib/api/types'
  import { api, ApiError } from '$lib/api/client'
  import Badge from '$components/Badge.svelte'
  import DataTable from '$components/DataTable.svelte'
  import EmptyState from '$components/EmptyState.svelte'

  let orgId = $derived(page.params.org_id)

  let tokens: EnrollmentToken[] = $state([])
  let loading = $state(true)
  let error: string | null = $state(null)

  function formatDate(ts: number | null): string {
    if (!ts) return 'Never'
    return new Date(ts).toLocaleDateString('en-US', {
      year: 'numeric',
      month: 'short',
      day: 'numeric',
    })
  }

  async function load(id: string) {
    loading = true
    error = null
    try {
      // Get all formations to fetch tokens per formation
      const formations: FormationConfig[] = await api.listFormations(id)
      const allTokens = await Promise.all(
        formations.map((f) => api.listTokens(id, f.app_id).catch(() => [])),
      )
      tokens = allTokens.flat()
    } catch (e) {
      error = e instanceof ApiError ? e.message : 'Failed to load tokens'
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
    <Password size={24} class="text-gray-400" />
    <h1 class="text-2xl font-semibold text-white">Enrollment Tokens</h1>
    <span class="text-sm text-gray-500">({orgId})</span>
  </div>

  {#if loading}
    <div class="card animate-pulse">
      <div class="h-40 rounded bg-gray-800"></div>
    </div>
  {:else if error}
    <div class="rounded-lg bg-red-900/30 p-4 text-red-200">{error}</div>
  {:else if tokens.length === 0}
    <EmptyState message="No enrollment tokens found" />
  {:else}
    <DataTable columns={['Label', 'App ID', 'Uses', 'Expires', 'Status', 'Created']}>
      {#each tokens as token (token.token_id)}
        <tr class="table-row">
          <td class="data-cell font-medium text-white">{token.label}</td>
          <td class="data-cell font-mono text-xs">{token.app_id}</td>
          <td class="data-cell">
            {token.uses}{token.max_uses !== null ? ` / ${token.max_uses}` : ''}
          </td>
          <td class="data-cell text-gray-500">{formatDate(token.expires_at)}</td>
          <td class="data-cell">
            {#if token.revoked}
              <Badge variant="red">Revoked</Badge>
            {:else if token.expires_at && token.expires_at < Date.now()}
              <Badge variant="yellow">Expired</Badge>
            {:else if token.max_uses !== null && token.uses >= token.max_uses}
              <Badge variant="gray">Exhausted</Badge>
            {:else}
              <Badge variant="green">Active</Badge>
            {/if}
          </td>
          <td class="data-cell text-gray-500">{formatDate(token.created_at)}</td>
        </tr>
      {/each}
    </DataTable>
  {/if}
</div>
