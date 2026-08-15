import { describe, expect, it } from 'vitest'

import type { ApiContentItem, ContentListResponse } from '@/api'

import {
	installedFacts,
	installedProjects,
	installState,
	isSelectable,
	stillSelectable,
} from './installed-projects'

function row(over: Partial<ApiContentItem>): ApiContentItem {
	return {
		id: '01J8Z0000000000000000ROW1',
		file_name: 'worldedit-bukkit-7.4.3.jar',
		file_path: '/plugins/worldedit-bukkit-7.4.3.jar',
		size: 1_000,
		enabled: true,
		locked: false,
		date_added: '2026-08-01T10:00:00Z',
		project_type: 'plugin',
		source_kind: 'modrinth',
		environment: null,
		pack_client_retained: false,
		pack_client_depends: false,
		installing: false,
		external: false,
		external_url: null,
		has_update: false,
		update_version_id: null,
		project_id: null,
		project: null,
		version: null,
		owner: null,
		...over,
	} as ApiContentItem
}

function listResponse(items: ApiContentItem[]): ContentListResponse {
	return {
		content_type: 'plugin',
		loader: 'paper',
		loader_version: null,
		game_version: '1.21.1',
		update_channel: 'release',
		updates_checked_at: null,
		permissions: { can_read: true, can_write: true },
		modpack: null,
		items,
		truncated: false,
	}
}

describe('installedProjects', () => {
	it('counts a row without a project card too', () => {
		const projects = installedProjects(listResponse([row({ project_id: '1u6JkXh5' })]))
		expect([...projects.installed]).toEqual(['1u6JkXh5'])
	})

	it('leaves out a row without an origin', () => {
		const projects = installedProjects(listResponse([row({})]))
		expect([...projects.installed]).toEqual([])
		expect([...projects.installing]).toEqual([])
	})

	it('names every project once and takes the modpack in as well', () => {
		const list = listResponse([
			row({ project_id: '1u6JkXh5' }),
			row({ id: '01J8Z0000000000000000ROW2', project_id: '1u6JkXh5' }),
		])
		list.modpack = { project_id: 'gHYUOczU' } as ContentListResponse['modpack']
		expect([...installedProjects(list).installed].sort()).toEqual(['1u6JkXh5', 'gHYUOczU'])
	})

	it('separates what is still loading from what is there', () => {
		const projects = installedProjects(
			listResponse([
				row({ project_id: '1u6JkXh5' }),
				row({ id: '01J8Z0000000000000000ROW2', project_id: 'Vebnzrzj', installing: true }),
			]),
		)
		expect([...projects.installed]).toEqual(['1u6JkXh5'])
		expect([...projects.installing]).toEqual(['Vebnzrzj'])
	})

	it('calls a project loading as soon as one of its rows is loading', () => {
		const projects = installedProjects(
			listResponse([
				row({ project_id: '1u6JkXh5' }),
				row({ id: '01J8Z0000000000000000ROW2', project_id: '1u6JkXh5', installing: true }),
			]),
		)
		expect([...projects.installed]).toEqual([])
		expect([...projects.installing]).toEqual(['1u6JkXh5'])
	})

	it('keeps both sets empty without a list', () => {
		expect(installedProjects(null)).toEqual({ installed: new Set(), installing: new Set() })
	})
})

describe('installedFacts', () => {
	it('changes when loading turns into installed', () => {
		const loading = installedProjects(
			listResponse([row({ project_id: '1u6JkXh5', installing: true })]),
		)
		const done = installedProjects(listResponse([row({ project_id: '1u6JkXh5' })]))
		expect(installedFacts(loading)).not.toBe(installedFacts(done))
	})

	it('stays the same when only the order turns around', () => {
		const left = installedProjects(
			listResponse([
				row({ project_id: '1u6JkXh5' }),
				row({ id: '01J8Z0000000000000000ROW2', project_id: 'Vebnzrzj' }),
			]),
		)
		const right = installedProjects(
			listResponse([
				row({ project_id: 'Vebnzrzj' }),
				row({ id: '01J8Z0000000000000000ROW2', project_id: '1u6JkXh5' }),
			]),
		)
		expect(installedFacts(left)).toBe(installedFacts(right))
	})
})

describe('installState', () => {
	const projects = {
		installed: new Set(['1u6JkXh5']),
		installing: new Set(['Vebnzrzj']),
	}

	it('says installed to a result that is already on the server', () => {
		expect(installState('1u6JkXh5', projects, false)).toBe('installed')
	})

	it('says loading to one that is being fetched right now', () => {
		expect(installState('Vebnzrzj', projects, false)).toBe('installing')
	})

	it('calls the rest available and selected', () => {
		expect(installState('fALzjamp', projects, false)).toBe('available')
		expect(installState('fALzjamp', projects, true)).toBe('selected')
	})

	it('does not let a selection overwrite what the server says', () => {
		expect(installState('1u6JkXh5', projects, true)).toBe('installed')
		expect(installState('Vebnzrzj', projects, true)).toBe('installing')
	})

	it('releases only what 8.7 can still take', () => {
		expect(isSelectable('available')).toBe(true)
		expect(isSelectable('selected')).toBe(true)
		expect(isSelectable('installed')).toBe(false)
		expect(isSelectable('installing')).toBe(false)
	})
})

describe('stillSelectable', () => {
	it('takes out of the selection whatever is installed by now or loading', () => {
		const selection = new Map([
			['1u6JkXh5', { id: '1u6JkXh5', name: 'WorldEdit' }],
			['Vebnzrzj', { id: 'Vebnzrzj', name: 'LuckPerms' }],
			['fALzjamp', { id: 'fALzjamp', name: 'Chunky' }],
		])
		const kept = stillSelectable(selection, {
			installed: new Set(['1u6JkXh5']),
			installing: new Set(['Vebnzrzj']),
		})
		expect([...kept.keys()]).toEqual(['fALzjamp'])
	})

	it('leaves a selection nothing touched complete', () => {
		const selection = new Map([['fALzjamp', { id: 'fALzjamp', name: 'Chunky' }]])
		const kept = stillSelectable(selection, { installed: new Set(), installing: new Set() })
		expect(kept.size).toBe(selection.size)
	})
})
