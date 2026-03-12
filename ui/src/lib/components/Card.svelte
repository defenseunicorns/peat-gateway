<script lang="ts">
  import type { Snippet } from 'svelte'

  type Props = {
    label: string
    value: string | number
    icon?: Snippet
    class?: string
  }

  let { label, value, icon, class: className }: Props = $props()
</script>

<div class="card flex items-center gap-4 {className ?? ''}">
  {#if icon}
    <div class="flex h-10 w-10 flex-shrink-0 items-center justify-center rounded-lg bg-gray-800 text-gray-400">
      {@render icon()}
    </div>
  {/if}
  <div>
    <p class="text-sm text-gray-400">{label}</p>
    <p class="text-2xl font-semibold text-white">{value}</p>
  </div>
</div>
