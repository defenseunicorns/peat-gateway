<script lang="ts">
  import type { Snippet } from 'svelte'

  type Props = {
    columns: string[]
    head?: Snippet
    children: Snippet
    class?: string
  }

  let { columns, head, children, class: className }: Props = $props()
</script>

<div class="table-container {className ?? ''}">
  <table class="w-full text-left text-sm text-gray-400">
    <thead class="table-head">
      {#if head}
        {@render head()}
      {:else}
        <tr>
          {#each columns as col}
            <th class="header-cell">{col}</th>
          {/each}
        </tr>
      {/if}
    </thead>
    <tbody>
      {@render children()}
    </tbody>
  </table>
</div>
