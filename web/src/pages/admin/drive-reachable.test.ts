import { readFileSync } from 'node:fs'
import { resolve } from 'node:path'

import { describe, expect, it } from 'vitest'

const ROOT = resolve(import.meta.dirname, '../../../..')
const PAGE = 'web/src/pages/admin/Drive.vue'
const SERVER = 'crates/craftpanel/src/drive/mod.rs'
const CLIENT = 'web/src/api/drive.ts'

const VIEWS = ['wide', 'narrow'] as const

type View = (typeof VIEWS)[number]

const SHOWN_BY: Record<string, string> = {
	username: 'at(index).username',
	google_email: 'at(index).google_email',
	state: 'stateLabel(index)',
	last_error: 'at(index).last_error',
	storage_limit_bytes: 'storageLabel(index)',
	storage_usage_bytes: 'storageLabel(index)',
	uploaded_today_bytes: 'dayLabel(index)',
	daily_upload_limit_bytes: 'dayLabel(index)',
	backups: 'backupsLabel(index)',
	backup_bytes: 'backupsLabel(index)',
	checked_at: 'checkedLabel(index)',
}

const OFF_THE_SCREEN: Record<string, string> = {
	user_id: 'names the row and says whose access the Disconnect button hands back',
}

function read(file: string): string {
	return readFileSync(resolve(ROOT, file), 'utf8')
}

function block(source: string, opening: string): string {
	const start = source.indexOf(opening)
	expect(start, `${opening} does not stand in the file`).toBeGreaterThan(-1)
	const from = start + opening.length
	return source.slice(from, source.indexOf('\n}', from))
}

function fieldsOfStruct(name: string): string[] {
	const body = block(read(SERVER), `struct ${name} {`)
	return [...body.matchAll(/^ {4}(?:pub(?:\([\w:]+\))? )?(\w+):/gm)].map((field) => field[1])
}

function fieldsOfInterface(name: string): string[] {
	const body = block(read(CLIENT), `export interface ${name} {`)
	return [...body.matchAll(/^\t(?:readonly )?(\w+)\??:/gm)].map((field) => field[1])
}

function slotOf(source: string, name: string): string {
	const opening = `<template #${name}`
	const start = source.indexOf(opening)
	expect(start, `${PAGE} has no <template #${name}>`).toBeGreaterThan(-1)
	let depth = 0
	let at = start
	while (at < source.length) {
		const opens = source.indexOf('<template', at)
		const closes = source.indexOf('</template>', at)
		expect(closes, `<template #${name}> in ${PAGE} is never closed`).toBeGreaterThan(-1)
		if (opens !== -1 && opens < closes) {
			depth += 1
			at = opens + '<template'.length
			continue
		}
		depth -= 1
		at = closes + '</template>'.length
		if (depth === 0) return source.slice(start, at)
	}
	return ''
}

function columnsOf(view: View): string[] {
	const source = read(PAGE)
	const start = source.indexOf('const columns = computed')
	expect(start, `${PAGE} builds no columns`).toBeGreaterThan(-1)
	const both = source.slice(start, source.indexOf('\n)', start))
	expect(both, `${PAGE} no longer takes the first list for the wide screen`).toMatch(
		/wide\.value\s*\?\s*\[/,
	)
	const narrow = both.indexOf(': [')
	expect(narrow, `${PAGE} builds one column list, not one per view`).toBeGreaterThan(-1)
	const part = view === 'wide' ? both.slice(0, narrow) : both.slice(narrow)
	return [...part.matchAll(/key: '(\w+)'/g)].map((hit) => hit[1])
}

function readAloudOn(view: View): string {
	const source = read(PAGE)
	const cells = columnsOf(view).map((key) => slotOf(source, `cell-${key}`))
	if (view === 'wide') return cells.join('\n')
	return [...cells, slotOf(source, 'row-below')].join('\n')
}

function whereItIsRead(shows: string): string {
	if (!shows.endsWith('Label(index)')) return shows
	const helper = shows.slice(0, shows.indexOf('('))
	return block(read(PAGE), `function ${helper}(index: number): string {`)
}

describe('the account line the server sends', () => {
	const fields = fieldsOfStruct('DriveOverview')

	it('stands field for field in this table', () => {
		expect([...Object.keys(SHOWN_BY), ...Object.keys(OFF_THE_SCREEN)].sort()).toEqual(
			[...fields].sort(),
		)
	})

	it('is known to the client under exactly the same names', () => {
		expect(fieldsOfInterface('DriveOverview').sort()).toEqual([...fields].sort())
	})

	it('is read where the page says it is read', () => {
		for (const [field, shows] of Object.entries(SHOWN_BY)) {
			expect(whereItIsRead(shows), `${shows} never reads ${field}`).toMatch(
				new RegExp(`\\b${field}\\b`),
			)
		}
	})
})

describe('the table of connected accounts', () => {
	it('carries the fold-out line on the narrow screen and only there', () => {
		expect(read(PAGE)).toContain(':row-below-visible="!wide"')
	})

	it.each(VIEWS)('puts every field in front of the reader on a %s screen', (view) => {
		const shown = readAloudOn(view)
		for (const [field, shows] of Object.entries(SHOWN_BY)) {
			expect(shown, `a ${view} screen shows no ${field} (${shows})`).toContain(shows)
		}
	})
})
