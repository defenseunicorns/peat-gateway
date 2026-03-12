<script lang="ts">
  import { page } from '$app/state'
  import DataConnected from 'carbon-icons-svelte/lib/DataConnected.svelte'

  import type { CdcSinkConfig } from '$lib/api/types'
  import { api, ApiError } from '$lib/api/client'
  import DataTable from '$components/DataTable.svelte'
  import EmptyState from '$components/EmptyState.svelte'
  import StatusDot from '$components/StatusDot.svelte'

  let orgId = $derived(page.params.org_id)

  let sinks: CdcSinkConfig[] = $state([])
  let loading = $state(true)
  let error: string | null = $state(null)

  function sinkTypeName(sink: CdcSinkConfig): string {
    if ('Nats' in sink.sink_type) return 'NATS'
    if ('Kafka' in sink.sink_type) return 'Kafka'
    if ('Webhook' in sink.sink_type) return 'Webhook'
    return 'Unknown'
  }

  function sinkTypeDetail(sink: CdcSinkConfig): string {
    if ('Nats' in sink.sink_type) return sink.sink_type.Nats.subject_prefix
    if ('Kafka' in sink.sink_type) return sink.sink_type.Kafka.topic
    if ('Webhook' in sink.sink_type) return sink.sink_type.Webhook.url
    return ''
  }

  function formatDate(ts: number): string {
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
      sinks = await api.listSinks(id)
    } catch (e) {
      error = e instanceof ApiError ? e.message : 'Failed to load sinks'
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
    <DataConnected size={24} class="text-gray-400" />
    <h1 class="text-2xl font-semibold text-white">CDC Sinks</h1>
    <span class="text-sm text-gray-500">({orgId})</span>
  </div>

  {#if loading}
    <div class="card animate-pulse">
      <div class="h-40 rounded bg-gray-800"></div>
    </div>
  {:else if error}
    <div class="rounded-lg bg-red-900/30 p-4 text-red-200">{error}</div>
  {:else if sinks.length === 0}
    <EmptyState message="No CDC sinks configured" />
  {:else}
    <DataTable columns={['Sink ID', 'Type', 'Target', 'Status', 'Created']}>
      {#each sinks as sink (sink.sink_id)}
        <tr class="table-row">
          <td class="data-cell font-mono text-xs text-white">{sink.sink_id}</td>
          <td class="data-cell">{sinkTypeName(sink)}</td>
          <td class="data-cell max-w-xs truncate text-xs">{sinkTypeDetail(sink)}</td>
          <td class="data-cell">
            <span class="flex items-center gap-2">
              <StatusDot color={sink.enabled ? 'green' : 'red'} />
              {sink.enabled ? 'Enabled' : 'Disabled'}
            </span>
          </td>
          <td class="data-cell text-gray-500">{formatDate(sink.created_at)}</td>
        </tr>
      {/each}
    </DataTable>
  {/if}
</div>
