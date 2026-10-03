import { unified } from 'unified'
import remarkParse from 'remark-parse'

export interface ReleaseNotesLanguageSection {
  language: string
  content: string
}

export interface LocalizedReleaseNotes {
  preamble: string
  sections: ReleaseNotesLanguageSection[]
  definitions: string[]
}

const LANGUAGE_MARKER = /^<!--[\t ]*lang:[\t ]*([a-z]{2,3}(?:-[a-z0-9]{2,8})*)[\t ]*-->$/iu
const parser = unified().use(remarkParse)

/** 只识别顶层独立注释；代码、引用、列表及 HTML 块内的示例不作为分段。 */
export function parseReleaseNotesLanguages(source: string): LocalizedReleaseNotes | null {
  const markers: Array<{ language: string; start: number; end: number }> = []
  const languages = new Set<string>()
  const nodes = parser.parse(source).children
  const definitions = nodes.filter((node) => node.type === 'definition')
    .map((node) => source.slice(node.position?.start.offset, node.position?.end.offset))
  for (const node of nodes) {
    if (node.type !== 'html') continue
    const marker = LANGUAGE_MARKER.exec(node.value.trim())
    if (!marker) {
      if (/^<!--\s*lang\b/iu.test(node.value.trim())
        || (!node.value.trim().startsWith('<!--') && /<!--\s*lang\b/iu.test(node.value))) return null
      continue
    }
    const start = node.position?.start.offset
    const end = node.position?.end.offset
    if (start === undefined || end === undefined) return null
    const lineStart = source.lastIndexOf('\n', start - 1) + 1
    const lineEnd = source.indexOf('\n', end)
    if (!/^ {0,3}$/u.test(source.slice(lineStart, start))
      || !/^[\t \r]*$/u.test(source.slice(end, lineEnd === -1 ? source.length : lineEnd))) {
      return null
    }
    const language = marker[1].toLowerCase()
    if (languages.has(language)) return null
    languages.add(language)
    markers.push({ language, start: lineStart, end: lineEnd === -1 ? end : lineEnd + 1 })
  }
  if (markers.length === 0) return null
  return {
    preamble: source.slice(0, markers[0].start),
    sections: markers.map((marker, index) => ({
      language: marker.language,
      content: source.slice(marker.end, markers[index + 1]?.start ?? source.length)
    })),
    definitions
  }
}

export function hasReleaseNotesContent(content: string): boolean {
  return parser.parse(content).children.some((node) => node.type !== 'html' && node.type !== 'definition')
}

/** 匹配语言优先，其次同语种、英文、首个非空版本；公共前言始终保留。 */
export function selectReleaseNotesLanguage(source: string, language: string): string {
  const parsed = parseReleaseNotesLanguages(source)
  if (!parsed) return source
  const sections = parsed.sections.filter((section) => hasReleaseNotesContent(section.content))
  const normalized = language.toLowerCase()
  const primary = normalized.split('-')[0]
  const selected = sections.find((section) => section.language === normalized)
    ?? sections.find((section) => section.language === primary)
    ?? sections.find((section) => section.language.split('-')[0] === primary)
    ?? sections.find((section) => section.language === 'en')
    ?? sections.find((section) => section.language.startsWith('en-'))
    ?? sections[0]
  if (!selected) return source
  const displayed = parsed.preamble + selected.content
  // 引用式链接定义属于整份 Markdown，不能随未选语言段一起丢失。
  const missingDefinitions = parsed.definitions.filter((definition) => !displayed.includes(definition))
  return missingDefinitions.length > 0
    ? `${displayed.trimEnd()}\n\n${missingDefinitions.join('\n')}\n`
    : displayed
}
