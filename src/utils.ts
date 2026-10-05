export function extractComicId(input: string): string | undefined {
  // 如果输入是纯数字 id，直接返回（保留字符串形式，与后端 `Comic.id` 一致）
  const trimmed = input.trim()
  if (/^\d+$/.test(trimmed)) {
    return trimmed
  }
  // 否则需要从链接中提取
  const regex = /aid-(\d+)/
  const match = input.match(regex)
  if (match === null || match[1] === null) {
    return
  }
  return match[1]
}
