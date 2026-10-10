/**
 * 事件日志里 JSON 载荷的紧凑渲染。
 *
 * 事件面板横向很窄、纵向很宽，所以排版目标是「少占行数、同时保持行短」：
 * - 标量子结构折叠成一行：`"local_addr": {"url": "tcp://0.0.0.0:11010"}`
 *   → `"local_addr":{"url":"tcp://0.0.0.0:11010"}`
 * - 单空格缩进、冒号后不加空格，省下的横向空间给 URL / uuid 这类长值
 *
 * 折叠受 {@link MAX_INLINE_WIDTH} 约束：整块拼起来超宽就保持展开。
 * 否则一条 PeerConnAdded 会被压成一行 250 字符 —— 行数是少了，但结构全糊在
 * 一行里反而更难读，超出预算的部分照样会在窄盒里折行。
 *
 * 只改排版、不改结构：单键包裹对象仍然保留原样，
 * 这样用户看到的形状和真实载荷对得上，复制出去也仍是合法 JSON。
 */

/** 缩进单位：1 空格即可看清层级，比默认的 2 空格省一半宽度。 */
const INDENT = ' '

/**
 * 单行折叠的宽度预算（不含缩进）。
 * 72 覆盖常见的 URL / uuid / stats 组合，再宽就会在事件面板里折行。
 */
export const MAX_INLINE_WIDTH = 72

export function formatCompactJson(value: unknown): string {
  // 防循环引用：JSON.stringify 会抛错，这里降级成占位符而不是整段失败
  const seen = new WeakSet<object>()

  const render = (node: unknown, depth: number): string => {
    if (node === undefined)
      return 'null'

    const pad = INDENT.repeat(depth)
    const padInner = INDENT.repeat(depth + 1)

    if (Array.isArray(node)) {
      if (node.length === 0)
        return '[]'
      if (seen.has(node))
        return '"[Circular]"'
      seen.add(node)

      // 先按子深度渲染，再用「拼起来有多宽」决定折叠还是展开 ——
      // 谓词和渲染共用同一份结果，不会出现「判定说能折叠、实际折行了」的错位
      const parts = node.map(item => render(item, depth + 1))
      const inline = `[${parts.join(',')}]`
      const out = parts.every(part => !part.includes('\n')) && inline.length <= MAX_INLINE_WIDTH
        ? inline
        : `[\n${parts.map(part => padInner + part).join(',\n')}\n${pad}]`
      seen.delete(node)
      return out
    }

    if (node && typeof node === 'object') {
      const entries = Object.entries(node as Record<string, unknown>)
      if (entries.length === 0)
        return '{}'
      if (seen.has(node))
        return '"[Circular]"'
      seen.add(node)

      const parts = entries.map(
        ([key, item]) => [JSON.stringify(key), render(item, depth + 1)] as const,
      )
      const inline = `{${parts.map(([key, part]) => `${key}:${part}`).join(',')}}`
      const out = parts.every(([, part]) => !part.includes('\n')) && inline.length <= MAX_INLINE_WIDTH
        ? inline
        : `{\n${parts.map(([key, part]) => `${padInner}${key}:${part}`).join(',\n')}\n${pad}}`
      seen.delete(node)
      return out
    }

    return JSON.stringify(node) ?? 'null'
  }

  try {
    return render(value, 0)
  }
  catch {
    // getter 抛错等极端情况，退回原始字符串，绝不让整个面板白屏
    return String(value)
  }
}