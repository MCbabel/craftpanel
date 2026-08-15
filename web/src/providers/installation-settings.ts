import type { Labrinth } from '@modrinth/api-client'
import {
	type BusyReason,
	commonMessages,
	type ContentDiffPreview,
	defineMessages,
	type GameVersionOption,
	type InstallationInfoRow,
	type InstallationModpackData,
	type InstallationSettingsContext,
	type LoaderVersionEntry,
	provideInstallationSettings,
	useVIntl,
} from '@modrinth/ui'
import type { ComputedRef, Ref } from 'vue'
import { computed, onScopeDispose, ref, watch } from 'vue'

import {
	api,
	type ContentListResponse,
	type GameVersionEntry,
	type LoaderBuild,
	type LoaderId,
	type LoaderInfo,
	type Operation,
	type Server,
	type ServerEventSource,
	type Ulid,
} from '@/api'

import {
	awaitOperation,
	type FilePickerRequest,
	isAbortError,
	type ModrinthVersionSource,
	type Notify,
	type OperationHost,
	type PanelApi,
	pickFilesFromDocument,
	useBusyState,
} from './content-manager'

const LOADER_FAMILY: Record<LoaderId, string> = {
	vanilla: 'vanilla',
	paper: 'bukkit',
	folia: 'bukkit',
	purpur: 'bukkit',
	leaf: 'bukkit',
	fabric: 'modloader',
	quilt: 'modloader',
	neoforge: 'modloader',
	forge: 'modloader',
	velocity: 'proxy',
}

const messages = defineMessages({
	loaderVersionLabel: {
		id: 'craftpanel.installation.loader-version',
		defaultMessage: '{loader} version',
	},
	loadFailed: {
		id: 'craftpanel.installation.load-failed',
		defaultMessage: 'Failed to load installation options',
	},
	saveFailed: {
		id: 'craftpanel.installation.save-failed',
		defaultMessage: 'Failed to change the installation',
	},
	previewFailed: {
		id: 'craftpanel.installation.preview-failed',
		defaultMessage: 'Failed to check which content survives the change',
	},
	unknownLoader: {
		id: 'craftpanel.installation.unknown-loader',
		defaultMessage: 'The loader catalogue is not loaded yet',
	},
	repairFailed: {
		id: 'craftpanel.installation.repair-failed',
		defaultMessage: 'Failed to repair the server',
	},
	modpackFailed: {
		id: 'craftpanel.installation.modpack-failed',
		defaultMessage: 'Failed to change the modpack',
	},
	versionsFailed: {
		id: 'craftpanel.installation.versions-failed',
		defaultMessage: 'Failed to load versions from Modrinth',
	},
	modpackLabel: {
		id: 'craftpanel.installation.modpack',
		defaultMessage: 'Modpack',
	},
	proxyIgnoresProperties: {
		id: 'craftpanel.installation.proxy-ignores-properties',
		defaultMessage: 'A proxy does not read server.properties; those settings will be ignored.',
	},
})

function buildKey(loader: string, gameVersion: string): string {
	return `${loader}\u0000${gameVersion}`
}

function isLoaderId(value: string, loaders: LoaderInfo[]): value is LoaderId {
	return loaders.some((loader) => loader.id === value)
}

export interface InstallationSettingsOptions {
	serverId: Ulid
	server: Readonly<Ref<Server | null>>
	socket: ServerEventSource
	busyReasons: Ref<BusyReason[]> | ComputedRef<BusyReason[]>
	notify: Notify
	modrinth: ModrinthVersionSource
	closeSettings?: () => void
	skipNonEssentialWarnings?: Ref<boolean>
	loaderWave?: 1 | 2
	pickFiles?: (request: FilePickerRequest) => Promise<File[]>
	client?: PanelApi
}

export function useInstallationSettings(options: InstallationSettingsOptions) {
	const { formatMessage } = useVIntl()
	const client = options.client ?? api
	const serverId = options.serverId
	const server = options.server
	const host: OperationHost = { serverId, socket: options.socket, client }
	const wave = options.loaderWave ?? 1
	const lifetime = new AbortController()

	const loaders = ref<LoaderInfo[]>([])
	const gameVersions = ref(new Map<string, GameVersionEntry[]>())
	const builds = ref(new Map<string, LoaderBuild[]>())
	const inFlight = new Set<string>()

	const content = ref<ContentListResponse | null>(null)
	const modpackVersions = ref<Labrinth.Versions.v2.Version[] | null>(null)
	const loadingCatalog = ref(true)
	const repairing = ref(false)
	const reinstalling = ref(false)

	const busy = useBusyState(options.busyReasons)
	const isBusy = computed(
		() =>
			busy.isBusy.value ||
			server.value?.status === 'installing' ||
			repairing.value ||
			reinstalling.value,
	)

	const currentPlatform = computed(() => server.value?.loader ?? 'vanilla')
	const currentGameVersion = computed(() => server.value?.game_version ?? '')
	const currentLoaderVersion = computed(() => server.value?.loader_version ?? '')

	const editingPlatform = ref(currentPlatform.value)
	const editingGameVersion = ref(currentGameVersion.value)

	const modpack = computed(() => content.value?.modpack ?? null)
	const modpackProjectId = computed(() => modpack.value?.project_id ?? null)

	function reportError(descriptor: (typeof messages)[keyof typeof messages], cause: unknown) {
		if (lifetime.signal.aborted || isAbortError(cause)) return
		options.notify({
			type: 'error',
			title: formatMessage(descriptor),
			text: cause instanceof Error ? cause.message : undefined,
		})
	}

	function loaderName(id: string): string | null {
		return loaders.value.find((loader) => loader.id === id)?.name ?? null
	}

	async function ensureGameVersions(loader: string) {
		if (gameVersions.value.has(loader) || inFlight.has(loader)) return
		if (!isLoaderId(loader, loaders.value)) return
		inFlight.add(loader)
		try {
			const response = await client.settings.gameVersions(loader, { signal: lifetime.signal })
			gameVersions.value.set(loader, response.game_versions)
		} catch (cause) {
			reportError(messages.loadFailed, cause)
		} finally {
			inFlight.delete(loader)
		}
	}

	async function ensureBuilds(loader: string, gameVersion: string) {
		if (loader === 'vanilla' || !gameVersion) return
		const key = buildKey(loader, gameVersion)
		if (builds.value.has(key) || inFlight.has(key)) return
		if (!isLoaderId(loader, loaders.value)) return
		inFlight.add(key)
		try {
			const response = await client.settings.builds(loader, gameVersion, {
				signal: lifetime.signal,
			})
			builds.value.set(key, response.builds)
		} catch (cause) {
			reportError(messages.loadFailed, cause)
		} finally {
			inFlight.delete(key)
		}
	}

	function resolveGameVersions(loader: string, showSnapshots: boolean): GameVersionOption[] {
		const entries = gameVersions.value.get(loader) ?? []
		return entries
			.filter((entry) => showSnapshots || entry.version_type === 'release')
			.map((entry) => ({ value: entry.version, label: entry.version }))
	}

	function resolveLoaderVersions(loader: string, gameVersion: string): LoaderVersionEntry[] {
		if (loader === 'vanilla' || !gameVersion) return []
		return (builds.value.get(buildKey(loader, gameVersion)) ?? []).map((build) => ({
			id: build.id,
			label: build.label,
			stable: build.stable,
			...(build.channel_tag ? { channelTag: build.channel_tag } : {}),
		}))
	}

	function resolveHasSnapshots(loader: string): boolean {
		return (gameVersions.value.get(loader) ?? []).some((entry) => entry.version_type === 'snapshot')
	}

	async function loadCatalog() {
		try {
			loaders.value = (await client.settings.loaders({ signal: lifetime.signal })).loaders
		} catch (cause) {
			reportError(messages.loadFailed, cause)
		} finally {
			loadingCatalog.value = false
		}
	}

	async function loadContent() {
		if (lifetime.signal.aborted) return
		try {
			content.value = await client.content.list(serverId, {}, { signal: lifetime.signal })
		} catch (cause) {
			reportError(messages.loadFailed, cause)
		}
	}

	async function runOperation(
		start: () => Promise<{ operation: Operation }>,
		failure: (typeof messages)[keyof typeof messages],
	) {
		const accepted = await start()
		const finished = await awaitOperation(host, accepted.operation, undefined, lifetime.signal)
		if (finished.state === 'failed') {
			options.notify({
				type: 'error',
				title: formatMessage(failure),
				text: finished.error?.message,
			})
		}
		await loadContent()
	}

	async function install(platform: string, gameVersion: string, loaderVersionId: string | null) {
		if (!isLoaderId(platform, loaders.value)) throw new Error(formatMessage(messages.unknownLoader))
		const sameFamily = LOADER_FAMILY[platform] === LOADER_FAMILY[currentPlatform.value]
		await runOperation(async () => {
			const accepted = await client.settings.install(serverId, {
				loader: platform,
				game_version: gameVersion,
				loader_version: platform === 'vanilla' ? null : loaderVersionId,
				content_policy: sameFamily ? 'keep' : 'wipe_mods',
			})
			if (accepted.warnings?.includes('properties_will_be_ignored')) {
				options.notify({
					type: 'warning',
					text: formatMessage(messages.proxyIgnoresProperties),
				})
			}
			return accepted
		}, messages.saveFailed)
	}

	async function save(platform: string, gameVersion: string, loaderVersionId: string | null) {
		if (isBusy.value) return
		const platformChanged = platform !== currentPlatform.value
		const gameVersionChanged = gameVersion !== currentGameVersion.value
		try {
			if (!platformChanged && gameVersionChanged && isLoaderId(platform, loaders.value)) {
				await runOperation(
					() =>
						client.content.changeGameVersion(serverId, {
							game_version: gameVersion,
							loader: platform,
							loader_version: loaderVersionId,
							incompatible_content: 'update_then_disable',
						}),
					messages.saveFailed,
				)
				return
			}
			await install(platform, gameVersion, loaderVersionId)
		} catch (cause) {
			reportError(messages.saveFailed, cause)
			throw cause
		}
	}

	async function saveWithoutAutoFix(
		platform: string,
		gameVersion: string,
		loaderVersionId: string | null,
	) {
		if (isBusy.value) return
		try {
			await install(platform, gameVersion, loaderVersionId)
		} catch (cause) {
			reportError(messages.saveFailed, cause)
			throw cause
		}
	}

	async function previewSave(
		platform: string,
		gameVersion: string,
		loaderVersionId: string | null,
		signal?: AbortSignal,
	): Promise<ContentDiffPreview | null> {
		if (!isLoaderId(platform, loaders.value)) return null
		const response = await client.content
			.previewGameVersion(
				serverId,
				{
					game_version: gameVersion,
					loader: platform,
					loader_version: loaderVersionId ?? undefined,
				},
				{ signal },
			)
			.catch((cause: unknown) => {
				reportError(messages.previewFailed, cause)
				throw cause
			})
		if (response.changes.length === 0 && !response.has_unknown_content) return null
		return {
			diffs: response.changes.map((change) => ({
				type: change.type,
				projectName: change.project_title ?? undefined,
				fileName: change.file_name ?? undefined,
				currentVersionName: change.current_version?.version_number,
				newVersionName: change.new_version?.version_number,
			})),
			newGameVersion: response.new_game_version,
			newLoaderVersion: response.new_loader_version ?? '',
			hasUnknownContent: response.has_unknown_content,
		}
	}

	function changeableContent() {
		return (content.value?.items ?? []).filter((item) => item.enabled && !item.locked)
	}

	async function disableAllContent() {
		const ids = changeableContent().map((item) => item.id)
		if (ids.length === 0) return
		await client.content.disable(serverId, { ids })
		await loadContent()
	}

	async function disableIncompatibleContent(targetGameVersion: string) {
		const active = changeableContent()
		const tracked = active.flatMap((item) =>
			item.version ? [{ id: item.id, versionId: item.version.id }] : [],
		)
		const incompatible = new Set(
			active.filter((item) => item.version === null).map((item) => item.id),
		)

		if (tracked.length > 0) {
			const versions = await options.modrinth.getVersions(tracked.map((entry) => entry.versionId))
			const unusable = new Set(
				versions
					.filter((version) => !version.game_versions.includes(targetGameVersion))
					.map((version) => version.id),
			)
			for (const entry of tracked) {
				if (unusable.has(entry.versionId)) incompatible.add(entry.id)
			}
		}

		if (incompatible.size === 0) return
		await client.content.disable(serverId, { ids: [...incompatible] })
		await loadContent()
	}

	async function repair() {
		if (isBusy.value) return
		repairing.value = true
		try {
			await runOperation(() => client.settings.repair(serverId), messages.repairFailed)
		} catch (cause) {
			reportError(messages.repairFailed, cause)
		} finally {
			repairing.value = false
		}
	}

	async function replaceLocalModpack(file: File) {
		if (modpack.value) await client.content.unlinkModpack(serverId)
		const handle = client.content.uploadModpack(serverId, file, false)
		const accepted = await handle.promise
		const finished = await awaitOperation(host, accepted.operation, undefined, lifetime.signal)
		if (finished.state === 'failed') {
			options.notify({
				type: 'error',
				title: formatMessage(messages.modpackFailed),
				text: finished.error?.message,
			})
		}
		await loadContent()
	}

	async function reinstallModpack() {
		if (isBusy.value || !modpack.value) return
		if (modpack.value.source_kind === 'local') {
			await swapModpack()
			return
		}
		reinstalling.value = true
		try {
			await runOperation(
				() =>
					client.content.updateModpack(serverId, {
						version_id: modpack.value?.version_id ?? null,
					}),
				messages.modpackFailed,
			)
		} catch (cause) {
			reportError(messages.modpackFailed, cause)
		} finally {
			reinstalling.value = false
		}
	}

	async function swapModpack() {
		if (isBusy.value) return
		const picked = await (options.pickFiles ?? pickFilesFromDocument)({
			accept: '.mrpack',
			multiple: false,
		})
		if (picked.length === 0) return
		reinstalling.value = true
		try {
			await replaceLocalModpack(picked[0])
		} catch (cause) {
			reportError(messages.modpackFailed, cause)
		} finally {
			reinstalling.value = false
		}
	}

	async function unlinkModpack() {
		if (isBusy.value) return
		try {
			await client.content.unlinkModpack(serverId)
		} catch (cause) {
			reportError(messages.modpackFailed, cause)
		} finally {
			await loadContent()
		}
	}

	async function fetchModpackVersions() {
		const projectId = modpackProjectId.value
		if (!projectId) throw new Error('No modpack linked')
		try {
			const versions = await options.modrinth.getProjectVersions(projectId, {
				include_changelog: false,
			})
			modpackVersions.value = versions
			return versions
		} catch (cause) {
			reportError(messages.versionsFailed, cause)
			throw cause
		}
	}

	async function onModpackVersionConfirm(version: Labrinth.Versions.v2.Version) {
		if (isBusy.value) return
		try {
			await runOperation(
				() => client.content.updateModpack(serverId, { version_id: version.id }),
				messages.modpackFailed,
			)
		} catch (cause) {
			reportError(messages.modpackFailed, cause)
		}
	}

	async function resetServer() {
		const loader = server.value?.loader
		const gameVersion = server.value?.game_version
		if (!loader || !gameVersion) return
		try {
			await runOperation(
				() =>
					client.settings.reset(serverId, {
						loader,
						game_version: gameVersion,
						loader_version: server.value?.loader_version ?? null,
						keep_backups: true,
					}),
				messages.saveFailed,
			)
		} catch (cause) {
			reportError(messages.saveFailed, cause)
		}
	}

	async function resetToSetup() {
		try {
			await client.settings.resetToSetup(serverId)
		} catch (cause) {
			reportError(messages.saveFailed, cause)
		}
	}

	watch(
		[loaders, editingPlatform, editingGameVersion, currentPlatform, currentGameVersion],
		() => {
			void ensureGameVersions(editingPlatform.value)
			void ensureBuilds(editingPlatform.value, editingGameVersion.value)
			void ensureGameVersions(currentPlatform.value)
			void ensureBuilds(currentPlatform.value, currentGameVersion.value)
		},
		{ immediate: true },
	)

	watch([currentPlatform, currentGameVersion], ([platform, gameVersion], [was, wasVersion]) => {
		if (editingPlatform.value === was) editingPlatform.value = platform
		if (editingGameVersion.value === wasVersion) editingGameVersion.value = gameVersion
	})

	const stopContentChanged = options.socket.on('content_changed', () => {
		void loadContent()
	})

	onScopeDispose(() => {
		stopContentChanged()
		lifetime.abort()
	})

	const installationInfo = computed<InstallationInfoRow[]>(() => {
		const pending = server.value === null
		const platform = currentPlatform.value
		const name = pending ? null : (loaderName(platform) ?? platform)
		const rows: InstallationInfoRow[] = [
			{ label: formatMessage(commonMessages.platformLabel), value: name },
			{
				label: formatMessage(commonMessages.gameVersionLabel),
				value: pending ? null : (server.value?.game_version ?? null),
			},
		]
		if (!pending && platform !== 'vanilla') {
			rows.push({
				label: formatMessage(messages.loaderVersionLabel, { loader: name ?? platform }),
				value: server.value?.loader_version ?? null,
			})
		}
		return rows
	})

	const modpackData = computed<InstallationModpackData | null>(() => {
		const pack = modpack.value
		if (!pack) return null
		const slugOrId = pack.slug ?? pack.project_id
		return {
			iconUrl: pack.icon_url ?? undefined,
			title: pack.title,
			link: slugOrId ? `https://modrinth.com/modpack/${encodeURIComponent(slugOrId)}` : undefined,
			versionNumber: pack.version_number ?? undefined,
			filename: pack.filename ?? undefined,
			owner: pack.owner
				? {
						id: pack.owner.id,
						name: pack.owner.name,
						type: pack.owner.type,
						iconUrl: pack.owner.avatar_url ?? undefined,
					}
				: undefined,
		}
	})

	const context: InstallationSettingsContext = {
		loading: computed(() => loadingCatalog.value || server.value === null),
		installationInfo,
		isLinked: computed(() => modpack.value !== null),
		isBusy,
		busyMessage: busy.busyMessage,
		skipNonEssentialWarnings: options.skipNonEssentialWarnings,
		modpack: modpackData,
		currentPlatform,
		currentGameVersion,
		currentLoaderVersion,
		availablePlatforms: computed(() =>
			loaders.value.filter((loader) => loader.wave <= wave).map((loader) => loader.id),
		),
		resolveGameVersions,
		resolveLoaderVersions,
		resolveHasSnapshots,
		onGameVersionHover: (option) => void ensureBuilds(editingPlatform.value, option.value),
		save,
		repair,
		reinstallModpack,
		swapModpack,
		unlinkModpack,
		getCachedModpackVersions: () => modpackVersions.value,
		fetchModpackVersions,
		getVersionChangelog: (versionId) => options.modrinth.getVersion(versionId).catch(() => null),
		onModpackVersionConfirm,
		updaterModalProps: computed(() => ({
			isApp: false,
			currentVersionId: modpack.value?.version_id ?? '',
			projectIconUrl: modpack.value?.icon_url ?? undefined,
			projectName: modpack.value?.title ?? formatMessage(messages.modpackLabel),
			currentGameVersion: currentGameVersion.value,
			currentLoader: currentPlatform.value,
		})),
		isServer: true,
		isApp: false,
		showModpackVersionActions: computed(() => modpack.value?.source_kind === 'modrinth_modpack'),
		isLocalFile: computed(() => modpack.value?.source_kind === 'local'),
		isManagedModpack: false,
		repairing,
		reinstalling,
		closeSettings: options.closeSettings,
		lockPlatform: false,
		hideLoaderVersion: false,
		disableAllContent,
		disableIncompatibleContent,
		saveWithoutAutoFix,
		previewSave,
		editingPlatformRef: editingPlatform,
		editingGameVersionRef: editingGameVersion,
	}

	provideInstallationSettings(context)
	void loadCatalog()
	void loadContent()

	return {
		context,
		loaders,
		editingPlatform,
		editingGameVersion,
		resetServer,
		resetToSetup,
		reload: loadContent,
	}
}
