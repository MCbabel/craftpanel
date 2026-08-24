import { readdirSync, readFileSync } from 'node:fs'
import { join, relative, resolve } from 'node:path'

import { IntlMessageFormat } from 'intl-messageformat'
import ts from 'typescript'
import { describe, expect, it } from 'vitest'

const SOURCE = resolve(import.meta.dirname, '..')
const ENGLISH = 'en-US'
const GERMAN = 'de-DE'
const TRANSLATIONS = [GERMAN]
const LANGUAGES = [ENGLISH, ...TRANSLATIONS]

const VOWELS = 'aeiouäöü'
const WORDS = /[A-Za-zÄÖÜäöüß]+/g
const SUBSTITUTES = /ae|oe|ue/g
const SPELLED_OUT = [
	'aero',
	'does',
	'goes',
	'guest',
	'israel',
	'michael',
	'poesie',
	'poet',
	'zue',
]
const SWISS = [
	'gross',
	'strass',
	'heiss',
	'weiss',
	'aussen',
	'ausser',
	'draussen',
	'schliess',
	'fliess',
	'geniess',
	'spass',
	'dreissig',
]

const TEXT = 0
const HASH = 7
const TAG = 8

type Template = { file: string; fallback: string | null }
type Entry = { defaultMessage: string }
type Part = {
	type: number
	value?: string
	options?: Record<string, { value: Part[] }>
	children?: Part[]
}

function files(folder: string): string[] {
	return readdirSync(folder, { withFileTypes: true }).flatMap((entry) => {
		const path = join(folder, entry.name)
		if (entry.isDirectory()) return files(path)
		return /\.(ts|vue)$/.test(entry.name) ? [path] : []
	})
}

function scripts(path: string): string[] {
	const text = readFileSync(path, 'utf8')
	if (!path.endsWith('.vue')) return [text]
	return [...text.matchAll(/<script\b[^>]*>([\s\S]*?)<\/script>/g)].map((hit) => hit[1])
}

function keyOf(field: ts.ObjectLiteralElementLike): string {
	const name = field.name
	if (name === undefined) return ''
	return ts.isIdentifier(name) || ts.isStringLiteralLike(name) ? name.text : ''
}

function fieldValue(node: ts.ObjectLiteralExpression, name: string): ts.Expression | null {
	const field = node.properties.find(
		(candidate): candidate is ts.PropertyAssignment =>
			ts.isPropertyAssignment(candidate) && keyOf(candidate) === name,
	)
	return field?.initializer ?? null
}

function fromTheSource() {
	const fixed = new Map<string, Template>()
	const built = new Set<string>()

	for (const path of files(SOURCE)) {
		for (const code of scripts(path)) {
			if (!code.includes('defaultMessage')) continue
			const tree = ts.createSourceFile(path, code, ts.ScriptTarget.Latest, true, ts.ScriptKind.TS)
			const walk = (node: ts.Node) => {
				if (ts.isObjectLiteralExpression(node)) {
					const id = fieldValue(node, 'id')
					const fallback = fieldValue(node, 'defaultMessage')
					if (id !== null && fallback !== null) {
						const text = ts.isStringLiteralLike(fallback) ? fallback.text : null
						if (ts.isStringLiteralLike(id)) {
							fixed.set(id.text, { file: relative(SOURCE, path), fallback: text })
						} else if (ts.isTemplateExpression(id) && id.head.text !== '') {
							built.add(id.head.text)
						}
					}
				}
				ts.forEachChild(node, walk)
			}
			walk(tree)
		}
	}

	return { fixed, built }
}

const { fixed, built } = fromTheSource()

function onlyAtRuntime(id: string): boolean {
	return [...built].some((start) => id.startsWith(start))
}

function catalogue(language: string): Record<string, Entry> {
	return JSON.parse(readFileSync(join(import.meta.dirname, language, 'index.json'), 'utf8'))
}

function placeholders(text: string, language: string): string[] {
	const found = new Set<string>()
	const walk = (parts: Part[]) => {
		for (const part of parts) {
			if (part.type === TEXT || part.type === HASH) continue
			if (part.type !== TAG && part.value !== undefined) found.add(part.value)
			for (const branch of Object.values(part.options ?? {})) walk(branch.value)
			walk(part.children ?? [])
		}
	}
	walk(new IntlMessageFormat(text, language).getAst() as Part[])
	return [...found].sort()
}

describe('The source', () => {
	it('writes its messages so that this test can count them', () => {
		expect(fixed.size).toBeGreaterThan(500)
	})
})

describe.each(LANGUAGES)('The catalogue %s', (language) => {
	const entries = catalogue(language)

	it('knows every message that stands in the source', () => {
		const missing = [...fixed.entries()]
			.filter(([id]) => !(id in entries))
			.map(([id, template]) => `${id} (${template.file})`)
			.sort()
		expect(missing, `untranslated in ${language}`).toEqual([])
	})

	it('carries no id that the source no longer has', () => {
		const orphaned = Object.keys(entries)
			.filter((id) => !fixed.has(id) && !onlyAtRuntime(id))
			.sort()
		expect(orphaned, `left over in ${language}`).toEqual([])
	})
})

describe(`The catalogue ${ENGLISH}`, () => {
	it('says word for word what stands in the source as the default', () => {
		const entries = catalogue(ENGLISH)
		const differing = [...fixed.entries()]
			.filter(([id, template]) => template.fallback !== null && id in entries)
			.filter(([id, template]) => entries[id].defaultMessage !== template.fallback)
			.map(([id, template]) => `${id} (${template.file})`)
			.sort()
		expect(differing).toEqual([])
	})
})

describe.each(TRANSLATIONS)('The translation %s', (language) => {
	const english = catalogue(ENGLISH)
	const entries = catalogue(language)

	it('sets the same placeholders as the English text', () => {
		const different = Object.entries(entries)
			.filter(([id]) => id in english)
			.map(([id, entry]) => ({
				id,
				here: placeholders(entry.defaultMessage, language),
				there: placeholders(english[id].defaultMessage, ENGLISH),
			}))
			.filter(({ here, there }) => here.join(' ') !== there.join(' '))
			.map(({ id, here, there }) => `${id}: ${there.join(', ')} → ${here.join(', ')}`)
		expect(different).toEqual([])
	})

	it('has the messages whose id only comes into being when it is shown as well', () => {
		const family = (entries: Record<string, Entry>) =>
			Object.keys(entries).filter(onlyAtRuntime).sort()
		expect(family(entries), `${language} against ${ENGLISH}`).toEqual(family(english))
	})
})

function writesAroundTheUmlaut(word: string): boolean {
	const lower = word.toLowerCase()
	if (SWISS.some((spelling) => lower.includes(spelling))) return true
	if (SPELLED_OUT.some((real) => lower.startsWith(real))) return false
	for (const hit of lower.matchAll(SUBSTITUTES)) {
		const at = hit.index
		const before = at === 0 ? '' : lower[at - 1]
		if (at + 2 === lower.length) continue
		if (lower.startsWith('uell', at)) continue
		if (hit[0] === 'ue' && before === 'q') continue
		if (before !== '' && VOWELS.includes(before)) continue
		return true
	}
	return false
}

describe(`The catalogue ${GERMAN}`, () => {
	it('writes the umlaut it means instead of spelling around it', () => {
		const entries = catalogue(GERMAN)
		const dodging = Object.entries(entries)
			.flatMap(([id, entry]) =>
				[...entry.defaultMessage.matchAll(WORDS)]
					.map((hit) => hit[0])
					.filter(writesAroundTheUmlaut)
					.map((word) => `${id}: ${word}`),
			)
			.sort()
		expect(
			dodging,
			'ä ö ü ß belong in the text; a word that is right anyway goes into SPELLED_OUT',
		).toEqual([])
	})
})
