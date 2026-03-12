<script lang="ts">
  import { page } from '$app/state'
  import Network_3 from 'carbon-icons-svelte/lib/Network_3.svelte'

  import type {
    CdcSinkConfig,
    FormationConfig,
    IdpConfig,
    Organization,
  } from '$lib/api/types'
  import { api, ApiError } from '$lib/api/client'
  import Badge from '$components/Badge.svelte'
  import Card from '$components/Card.svelte'
  import DataTable from '$components/DataTable.svelte'
  import EmptyState from '$components/EmptyState.svelte'
  import StatusDot from '$components/StatusDot.svelte'

  let orgId = $derived(page.params.org_id)

  let org: Organization | null = $state(null)
  let formations: FormationConfig[] = $state([])
  let sinks: CdcSinkConfig[] = $state([])
  let idps: IdpConfig[] = $state([])
  let loading = $state(true)
  let error: string | null = $state(null)

  async function load(id: string) {
    loading = true
    error = null
    try {
      const [o, f, s, i] = await Promise.all([
        api.getOrg(id),
        api.listFormations(id),
        api.listSinks(id),
        api.listIdps(id),
      ])
      org = o
      formations = f
      sinks = s
      idps = i
    } catch (e) {
      error = e instanceof ApiError ? e.message : 'Failed to load organization'
    } finally {
      loading = false
    }
  }

  $effect(() => {
    if (orgId) load(orgId)
  })
</script>

{#if loading}
  <div class="animate-pulse space-y-4">
    <div class="h-8 w-48 rounded bg-gray-800"></div>
    <div class="grid gap-4 md:grid-cols-3">
      {#each Array(3) as _}
        <div class="card h-20"></div>
      {/each}
    </div>
  </div>
{:else if error}
  <div class="rounded-lg bg-red-900/30 p-4 text-red-200">{error}</div>
{:else if org}
  <div class="mb-6">
    <h1 class="text-2xl font-semibold text-white">{org.display_name || org.org_id}</h1>
    <p class="text-sm text-gray-500">{org.org_id}</p>
  </div>

  <div class="mb-6 grid gap-4 md:grid-cols-4">
    <Card label="Formations" value={formations.length}>
      {#snippet icon()}
        <Network_3 size={20} />
      {/snippet}
    </Card>
    <Card label="CDC Sinks" value={sinks.length} />
    <Card label="IdPs" value={idps.length} />
    <Card
      label="Max Peers/Formation"
      value={org.quotas.max_peers_per_formation}
    />
  </div>

  <!-- Formations table -->
  <div class="mb-6">
    <h2 class="mb-3 text-lg font-medium text-white">Formations</h2>
    {#if formations.length === 0}
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
                View
              </a>
            </td>
          </tr>
        {/each}
      </DataTable>
    {/if}
  </div>

  <!-- IdP status -->
  <div class="mb-6">
    <h2 class="mb-3 text-lg font-medium text-white">Identity Providers</h2>
    {#if idps.length === 0}
      <EmptyState message="No IdPs configured" />
    {:else}
      <DataTable columns={['IdP ID', 'Issuer URL', 'Status']}>
        {#each idps as idp (idp.idp_id)}
          <tr class="table-row">
            <td class="data-cell font-medium text-white">{idp.idp_id}</td>
            <td class="data-cell truncate max-w-xs">{idp.issuer_url}</td>
            <td class="data-cell">
              <span class="flex items-center gap-2">
                <StatusDot color={idp.enabled ? 'green' : 'red'} />
                {idp.enabled ? 'Enabled' : 'Disabled'}
              </span>
            </td>
          </tr>
        {/each}
      </DataTable>
    {/if}
  </div>

  <!-- Quotas -->
  <div>
    <h2 class="mb-3 text-lg font-medium text-white">Quotas</h2>
    <div class="card grid gap-4 sm:grid-cols-2 lg:grid-cols-3">
      {#each [
        { label: 'Max Formations', value: org.quotas.max_formations, current: formations.length },
        { label: 'Max Peers/Formation', value: org.quotas.max_peers_per_formation },
        { label: 'Max Documents/Formation', value: org.quotas.max_documents_per_formation },
        { label: 'Max CDC Sinks', value: org.quotas.max_cdc_sinks, current: sinks.length },
        { label: 'Max Enrollments/Hour', value: org.quotas.max_enrollments_per_hour },
      ] as quota}
        <div>
          <p class="text-xs text-gray-500">{quota.label}</p>
          <p class="text-lg font-medium text-white">
            {#if quota.current !== undefined}
              {quota.current}
              <span class="text-sm text-gray-500">/ {quota.value}</span>
            {:else}
              {quota.value}
            {/if}
          </p>
        </div>
      {/each}
    </div>
  </div>
{/if}
