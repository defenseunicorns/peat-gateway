<script lang="ts">
  import { page } from '$app/state'
  import Network_3 from 'carbon-icons-svelte/lib/Network_3.svelte'

  import type { FormationConfig } from '$lib/api/types'
  import { api, ApiError } from '$lib/api/client'
  import Badge from '$components/Badge.svelte'
  import DataTable from '$components/DataTable.svelte'
  import EmptyState from '$components/EmptyState.svelte'

  let orgId = $derived(page.params.org_id)

  let formations: FormationConfig[] = $state([])
  let loading = $state(true)
  let error: string | null = $state(null)

  async function load(id: string) {
    loading = true
    error = null
    try {
      formations = await api.listFormations(id)
    } catch (e) {
      error = e instanceof ApiError ? e.message : 'Failed to load formations'
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
    <Network_3 size={24} class="text-gray-400" />
    <h1 class="text-2xl font-semibold text-white">Formations</h1>
    <span class="text-sm text-gray-500">({orgId})</span>
  </div>

  {#if loading}
    <div class="card animate-pulse">
      <div class="h-40 rounded bg-gray-800"></div>
    </div>
  {:else if error}
    <div class="rounded-lg bg-red-900/30 p-4 text-red-200">{error}</div>
  {:else if formations.length === 0}
    <EmptyState message="No formations in this organization" />
  {:else}
    <DataTable columns={['App ID', 'Mesh ID', 'Enrollment Policy', '']}>
      {#each formations as f (f.app_id)}
        <tr class="table-row table-row--hoverable">
          <td class="data-cell font-medium text-white">
            <a href="/_/orgs/{orgId}/formations/{f.app_id}" class="hover:text-blue-400">
              {f.app_id}
            </a>
          </td>
          <td class="data-cell font-mono text-xs">{f.mesh_id}</td>
          <td class="data-cell">
            <Badge policy={f.enrollment_policy}>{f.enrollment_policy}</Badge>
          </td>
          <td class="data-cell text-right">
            <a
              href="/_/orgs/{orgId}/formations/{f.app_id}"
              class="text-xs text-blue-400 hover:underline"
            >
              Details
            </a>
          </td>
        </tr>
      {/each}
    </DataTable>
  {/if}
</div>
