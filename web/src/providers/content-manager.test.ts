import { describe, expect, it, vi } from 'vitest'
import { effectScope, ref } from 'vue'

import {
	type ApiContentItem,
	type ApiFileItem,
	ApiRequestError,
	type ContentListResponse,
	type ContentModpack,
	type ServerEventSource,
} from '@/api'

import {
	type ModrinthVersionSource,
	type PanelApi,
	preselectedVersionId,
	type UpdaterModalHandle,
	updaterPlan,
	useContentManager,
} from './content-manager'

vi.mock('@modrinth/ui', () => ({
	defineMessages: (descriptors: Record<string, unknown>) => descriptors,
	provideContentManager: () => undefined,
	useVIntl: () => ({ formatMessage: (descriptor: { id: string }) => descriptor.id }),
}))

const OLD = '1111111111111111111111111111111111111111'
const NEW = '2222222222222222222222222222222222222222'

function row(over: Partial<ApiContentItem> = {}): ApiContentItem {
	return {
		id: '01J8Z0000000000000000ROW1',
		file_name: 'sodium-0.5.8.jar',
		file_path: 'mods/sodium-0.5.8.jar',
		size: 412_000,
		enabled: true,
		locked: false,
		date_added: '2026-08-01T10:00:00Z',
		project_type: 'mod',
		source_kind: 'modrinth',
		environment: null,
		pack_client_retained: false,
		pack_client_depends: false,
		installing: false,
		external: false,
		external_url: null,
		has_update: true,
		update_version_id: NEW,
		project_id: 'AANobbMI',
		project: { id: 'AANobbMI', slug: 'sodium', title: 'Sodium', icon_url: null },
		version: {
			id: OLD,
			version_number: '0.5.8',
			file_name: 'sodium-0.5.8.jar',
			date_published: '2026-07-01T10:00:00Z',
		},
		owner: null,
		...over,
	} as ApiContentItem
}

function pack(over: Partial<ContentModpack> = {}): ContentModpack {
	return {
		source_kind: 'modrinth_modpack',
		project_id: 'gHYUOczU',
		slug: 'fabulously-optimized',
		title: 'Fabulously Optimized',
		description: null,
		icon_url: null,
		filename: 'fo.mrpack',
		downloads: null,
		followers: null,
		owner: null,
		categories: [],
		version_id: OLD,
		version_number: '6.4.1',
		date_published: '2026-07-01T10:00:00Z',
		has_update: true,
		update_version_id: NEW,
		...over,
	}
}

function listResponse(over: Partial<ContentListResponse> = {}): ContentListResponse {
	return {
		content_type: 'mod',
		loader: 'fabric',
		loader_version: null,
		game_version: '1.21.1',
		update_channel: 'release',
		updates_checked_at: '2026-08-12T10:00:00Z',
		permissions: { can_read: true, can_write: true },
		modpack: null,
		items: [row()],
		truncated: false,
		...over,
	}
}

const silentSocket: ServerEventSource = {
	status: { state: 'closed', attempt: 0 },
	on: () => () => undefined,
} as unknown as ServerEventSource

function mount(response: ContentListResponse, directories: Record<string, ApiFileItem[]> = {}) {
	const show = vi.fn<UpdaterModalHandle['show']>()
	const updaterModal = ref<UpdaterModalHandle>({ show, hide: vi.fn() })
	const listAll = vi.fn(async (_server: string, path: string) => {
		const items = directories[path]
		if (items === undefined) throw new ApiRequestError(404, 'not_found', 'no such path')
		return { path, items, truncated: false }
	})
	const client = {
		content: { list: vi.fn().mockResolvedValue(response) },
		files: { listAll },
	} as unknown as PanelApi
	const modrinth: ModrinthVersionSource = {
		getProjectVersions: vi.fn().mockResolvedValue([]),
		getVersion: vi.fn(),
		getVersions: vi.fn().mockResolvedValue([]),
		getVersionFromFileHash: vi.fn(),
	}

	const notify = vi.fn()
	const scope = effectScope()
	const manager = scope.run(() =>
		useContentManager({
			serverId: '01J8Z00000000000000SERVER',
			socket: silentSocket,
			busyReasons: { value: [] } as never,
			notify,
			browse: () => undefined,
			modrinth,
			fileLink: (path, editing) => ({
				name: 'server-files',
				query: editing === undefined ? { path } : { path, editing },
			}),
			updaterModal,
			client,
		}),
	)!

	return { manager, show, notify, modrinth, listAll, scope }
}

async function settle() {
	for (let step = 0; step < 6; step += 1) await Promise.resolve()
}

describe('preselectedVersionId', () => {
	it('picks the new version when an update is there', () => {
		const chosen = preselectedVersionId(
			{ has_update: true, update_version_id: NEW, version_id: OLD },
			false,
		)
		expect(chosen).toBe(NEW)
		expect(chosen).not.toBe(OLD)
	})

	it('falls back to the installed version when no new one is named', () => {
		expect(
			preselectedVersionId({ has_update: true, update_version_id: null, version_id: OLD }, false),
		).toBe(OLD)
	})

	it('takes the installed version when switching versions', () => {
		expect(
			preselectedVersionId({ has_update: true, update_version_id: NEW, version_id: OLD }, true),
		).toBe(OLD)
	})

	it('stays with the installed version without an update', () => {
		expect(
			preselectedVersionId({ has_update: false, update_version_id: NEW, version_id: OLD }, false),
		).toBe(OLD)
	})

	it('delivers nothing when no version is known', () => {
		expect(
			preselectedVersionId({ has_update: false, update_version_id: null, version_id: null }, false),
		).toBeUndefined()
	})
})

describe('updaterPlan', () => {
	it('takes the project id of the row even when the project card is missing', () => {
		expect(updaterPlan(row({ project: null }), false)).toEqual({
			open: true,
			projectId: 'AANobbMI',
			initialVersionId: NEW,
		})
	})

	it('finds the installed version on the row as on the modpack', () => {
		expect(updaterPlan(row(), true)).toEqual({
			open: true,
			projectId: 'AANobbMI',
			initialVersionId: OLD,
		})
		expect(updaterPlan(pack(), true)).toEqual({
			open: true,
			projectId: 'gHYUOczU',
			initialVersionId: OLD,
		})
	})

	it('declines with a reason when the row belongs to no project', () => {
		expect(updaterPlan(row({ project_id: null, project: null }), false)).toEqual({
			open: false,
			reason: 'without-project',
		})
		expect(updaterPlan(pack({ project_id: null }), false)).toEqual({
			open: false,
			reason: 'without-project',
		})
	})

	it('declines with a reason when the row is gone', () => {
		expect(updaterPlan(undefined, false)).toEqual({ open: false, reason: 'gone' })
		expect(updaterPlan(null, true)).toEqual({ open: false, reason: 'gone' })
	})
})

describe('openUpdater', () => {
	it('opens the dialog on the new version, not on the installed one', async () => {
		const { manager, show, scope } = mount(listResponse())
		await settle()

		manager.context.updateItem?.(row().id)
		await settle()

		expect(show).toHaveBeenCalledTimes(1)
		expect(show.mock.calls[0][0]).toBe(NEW)
		expect(show.mock.calls[0][1]).toEqual({ switchMode: false })
		scope.stop()
	})

	it('opens the version switch on the installed version', async () => {
		const { manager, show, scope } = mount(listResponse())
		await settle()

		manager.context.switchVersion?.(manager.context.items.value[0])
		await settle()

		expect(show).toHaveBeenCalledTimes(1)
		expect(show.mock.calls[0][0]).toBe(OLD)
		expect(show.mock.calls[0][1]).toEqual({ switchMode: true })
		scope.stop()
	})

	it('keeps the installed version when the row names no new one', async () => {
		const { manager, show, scope } = mount(
			listResponse({ items: [row({ has_update: false, update_version_id: null })] }),
		)
		await settle()

		manager.context.updateItem?.(row().id)
		await settle()

		expect(show.mock.calls[0][0]).toBe(OLD)
		scope.stop()
	})

	it('opens the modpack on the new version', async () => {
		const { manager, show, scope } = mount(listResponse({ modpack: pack() }))
		await settle()

		manager.context.updateModpack?.()
		await settle()

		expect(show).toHaveBeenCalledTimes(1)
		expect(show.mock.calls[0][0]).toBe(NEW)
		scope.stop()
	})

	it('opens the dialog even when the project card is missing', async () => {
		const { manager, show, modrinth, scope } = mount(
			listResponse({ items: [row({ project: null })] }),
		)
		await settle()

		manager.context.updateItem?.(row().id)
		await settle()

		expect(show).toHaveBeenCalledTimes(1)
		expect(show.mock.calls[0][0]).toBe(NEW)
		expect(modrinth.getProjectVersions).toHaveBeenCalledWith('AANobbMI', {
			include_changelog: false,
		})
		scope.stop()
	})

	it('allows linking and switching the version even without a project card', async () => {
		const { manager, scope } = mount(listResponse({ items: [row({ project: null })] }))
		await settle()

		const mapped = manager.context.mapToTableItem(manager.context.items.value[0])
		expect(mapped.projectLink).toBe('https://modrinth.com/mod/AANobbMI')
		expect(mapped.hideSwitchVersion).toBe(false)
		scope.stop()
	})
})

describe('A refusal instead of a silent exit', () => {
	const orphan = row({ project_id: null, project: null })

	it('names on the update the reason why there is no version list', async () => {
		const { manager, show, notify, modrinth, scope } = mount(listResponse({ items: [orphan] }))
		await settle()

		manager.context.updateItem?.(orphan.id)
		await settle()

		expect(show).not.toHaveBeenCalled()
		expect(modrinth.getProjectVersions).not.toHaveBeenCalled()
		expect(notify).toHaveBeenCalledTimes(1)
		expect(notify.mock.calls[0][0]).toMatchObject({
			type: 'warning',
			title: 'craftpanel.content.no-version-list',
			text: 'craftpanel.content.without-project',
		})
		scope.stop()
	})

	it('names it on the version switch just the same', async () => {
		const { manager, show, notify, scope } = mount(listResponse({ items: [orphan] }))
		await settle()

		manager.context.switchVersion?.(manager.context.items.value[0])
		await settle()

		expect(show).not.toHaveBeenCalled()
		expect(notify.mock.calls[0][0].text).toBe('craftpanel.content.without-project')
		scope.stop()
	})

	it('names it on the modpack just the same', async () => {
		const { manager, show, notify, scope } = mount(
			listResponse({ modpack: pack({ project_id: null }) }),
		)
		await settle()

		manager.context.updateModpack?.()
		await settle()

		expect(show).not.toHaveBeenCalled()
		expect(notify.mock.calls[0][0].text).toBe('craftpanel.content.without-project')
		scope.stop()
	})

	it('reports a row that is gone instead of doing nothing', async () => {
		const { manager, show, notify, scope } = mount(listResponse())
		await settle()

		manager.context.updateItem?.('01J8Z0000000000000000GONE')
		await settle()

		expect(show).not.toHaveBeenCalled()
		expect(notify.mock.calls[0][0].text).toBe('craftpanel.content.entry-gone')
		scope.stop()
	})
})

describe('The button into the config folder', () => {
	function entry(name: string, type: ApiFileItem['type']): ApiFileItem {
		return { name, type, path: `/config/${name}`, modified: 0, created: 0 }
	}

	it('reads the config folder once, even with several rows', async () => {
		const { listAll, scope } = mount(
			listResponse({
				items: [row(), row({ id: '01J8Z0000000000000000ROW2', file_name: 'lithium.jar' })],
			}),
			{ '/config': [entry('sodium.json', 'file')] },
		)
		await settle()

		expect(listAll.mock.calls.map((call) => call[1])).toEqual(['/config'])
		scope.stop()
	})

	it('leads to the file the mod wrote', async () => {
		const { manager, scope } = mount(listResponse(), { '/config': [entry('sodium.json', 'file')] })
		await settle()

		const [option] = manager.context.getOverflowOptions?.(manager.context.items.value[0]) ?? []
		expect(option).toMatchObject({
			type: 'link',
			label: 'craftpanel.content.config-own',
			to: { name: 'server-files', query: { path: '/config', editing: 'sodium.json' } },
		})
		scope.stop()
	})

	it('leads into the shared folder when the mod has nothing of its own', async () => {
		const { manager, scope } = mount(listResponse(), { '/config': [entry('fabric', 'directory')] })
		await settle()

		const [option] = manager.context.getOverflowOptions?.(manager.context.items.value[0]) ?? []
		expect(option).toMatchObject({
			type: 'link',
			label: 'craftpanel.content.config-shared',
			to: { name: 'server-files', query: { path: '/config' } },
		})
		scope.stop()
	})

	it('says before the first start that there is nothing yet', async () => {
		const { manager, scope } = mount(listResponse())
		await settle()

		const [option] = manager.context.getOverflowOptions?.(manager.context.items.value[0]) ?? []
		expect(option).toMatchObject({ label: 'craftpanel.content.config-none', disabled: true })
		expect(option).not.toHaveProperty('to')
		scope.stop()
	})

	it('offers none at all to a datapack', async () => {
		const { manager, scope } = mount(
			listResponse({
				content_type: 'datapack',
				loader: 'vanilla',
				items: [
					row({
						file_name: 'vanilla-tweaks.zip',
						file_path: 'world/datapacks/vanilla-tweaks.zip',
						project_type: 'datapack',
					}),
				],
			}),
			{ '/world/datapacks': [entry('vanilla-tweaks.zip', 'file')] },
		)
		await settle()

		expect(manager.context.getOverflowOptions?.(manager.context.items.value[0])).toEqual([])
		scope.stop()
	})
})
