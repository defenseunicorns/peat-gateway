<script lang="ts">
  type Props = {
    color?: 'green' | 'yellow' | 'red' | 'gray'
    class?: string
  }

  let { color = 'gray', class: className }: Props = $props()

  const colors: Record<string, string> = {
    green: 'bg-green-500',
    yellow: 'bg-yellow-500',
    red: 'bg-red-500',
    gray: 'bg-gray-500',
  }
</script>

<span class="inline-block h-2.5 w-2.5 rounded-full {colors[color]} {className ?? ''}"></span>
