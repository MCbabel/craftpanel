<template>
	<div class="mx-auto flex w-full max-w-[42rem] flex-col gap-4">
		<div class="flex items-center gap-3">
			<ButtonLink :to="{ name: 'servers' }" type="quiet" size="sm">
				<LeftArrowIcon aria-hidden="true" />
				{{ formatMessage(commonMessages.serversLabel) }}
			</ButtonLink>
		</div>

		<h1 class="m-0 text-2xl font-extrabold text-contrast">{{ formatMessage(messages.title) }}</h1>

		<Admonition
			v-if="stage === 'form' && blocked"
			type="warning"
			:header="formatMessage(messages.blockedHeader)"
			:body="blocked"
		/>

		<template v-else-if="stage === 'form'">
			<LoadingIndicator v-if="catalogLoading" />

			<Admonition
				v-else-if="catalogFailure"
				type="critical"
				:header="formatMessage(messages.catalogFailed)"
			>
				{{ catalogFailure }}
				<template #actions>
					<Button @click="() => loadCatalog()">
						<RotateCounterClockwiseIcon aria-hidden="true" />
						{{ formatMessage(messages.retry) }}
					</Button>
				</template>
			</Admonition>

			<form v-else class="flex flex-col gap-4" @submit.prevent="create">
				<Card class="!mb-0 flex flex-col gap-5">
					<div class="flex flex-col gap-1.5">
						<label class="text-sm font-semibold text-contrast" for="server-name">
							{{ formatMessage(messages.nameLabel) }}
						</label>
						<StyledInput
							id="server-name"
							v-model="name"
							:maxlength="64"
							:placeholder="formatMessage(messages.namePlaceholder)"
							:disabled="busy"
							wrapper-class="w-full"
						/>
					</div>

					<div class="flex flex-col gap-2">
						<span class="text-sm font-semibold text-contrast">
							{{ formatMessage(commonMessages.platformLabel) }}
						</span>
						<Chips
							v-model="loader"
							:items="loaderIds"
							:format-label="loaderLabel"
							:disabled-items="unwiredLoaders"
							:disabled-tooltip="formatMessage(messages.loaderNotWired)"
							:capitalize="false"
							:aria-label="formatMessage(commonMessages.platformLabel)"
						/>
					</div>

					<div class="flex flex-col gap-2">
						<span class="text-sm font-semibold text-contrast">
							{{ formatMessage(commonMessages.gameVersionLabel) }}
						</span>
						<Combobox
							v-model="gameVersion"
							searchable
							:options="versionOptions"
							:disabled="busy"
							:aria-label="formatMessage(commonMessages.gameVersionLabel)"
							:placeholder="formatMessage(commonMessages.selectVersionPlaceholder)"
							:search-placeholder="formatMessage(commonMessages.searchVersionPlaceholder)"
							:no-options-message="
								versionsLoading
									? formatMessage(commonMessages.loadingLabel)
									: formatMessage(messages.noVersions)
							"
						>
							<template #dropdown-footer>
								<button
									type="button"
									class="flex w-full cursor-pointer items-center justify-center gap-1.5 border-0 border-t border-solid border-surface-5 bg-transparent py-3 text-center text-sm font-semibold text-secondary transition-colors hover:text-contrast"
									@mousedown.prevent
									@click="showSnapshots = !showSnapshots"
								>
									<EyeOffIcon v-if="showSnapshots" class="size-4" />
									<EyeIcon v-else class="size-4" />
									{{
										showSnapshots
											? formatMessage(commonMessages.hideSnapshotsButton)
											: formatMessage(commonMessages.showAllVersionsButton)
									}}
								</button>
							</template>
						</Combobox>
						<span v-if="versionsFailure" class="text-sm text-red">{{ versionsFailure }}</span>
					</div>

					<div v-if="selectedLoader?.has_loader_versions" class="flex flex-col gap-2">
						<span class="text-sm font-semibold text-contrast">
							{{ formatMessage(messages.buildLabel) }}
						</span>
						<Combobox
							v-model="build"
							searchable
							:options="buildOptions"
							:disabled="busy || !gameVersion"
							:aria-label="formatMessage(messages.buildLabel)"
							:placeholder="formatMessage(messages.latestStable)"
							:search-placeholder="formatMessage(commonMessages.searchVersionPlaceholder)"
							:no-options-message="
								buildsLoading
									? formatMessage(commonMessages.loadingLabel)
									: formatMessage(messages.noBuilds)
							"
						>
							<template v-if="channelled" #option-suffix="{ item }">
								<PaperChannelBadge :channel="channelOf(item.value)" />
							</template>
							<template v-if="channelled" #search-selection-affix="{ option }">
								<PaperChannelBadge affix :channel="option ? channelOf(option.value) : null" />
							</template>
						</Combobox>
						<span v-if="buildsFailure" class="text-sm text-red">{{ buildsFailure }}</span>
						<span v-else-if="buildsTruncated" class="text-sm text-secondary">
							{{ formatMessage(messages.buildsTruncated) }}
						</span>
					</div>
				</Card>

				<Card class="!mb-0 flex flex-col gap-3">
					<div class="flex flex-wrap items-baseline gap-2">
						<span class="text-sm font-semibold text-contrast">
							{{ formatMessage(messages.memoryLabel) }}
						</span>
						<span v-if="!unlimited" class="ml-auto text-sm text-secondary">
							{{
								formatMessage(messages.memoryBudget, {
									free: formatBytes(budgetLeft * 1024 * 1024, 0),
								})
							}}
						</span>
					</div>

					<LoadingIndicator v-if="ceiling === null" />
					<Admonition
						v-else-if="ceiling < MEMORY_MIN"
						type="warning"
						:body="formatMessage(messages.noMemoryLeft)"
					/>
					<Slider
						v-else
						v-model="memory"
						:min="MEMORY_MIN"
						:max="ceiling"
						:step="MEMORY_STEP"
						:disabled="busy"
						unit="MiB"
					/>

					<Admonition
						v-if="capacity.fallback"
						type="warning"
						:body="formatMessage(messages.hostUnavailable)"
					>
						<template #actions>
							<Button @click="() => loadHost()">
								<RotateCounterClockwiseIcon aria-hidden="true" />
								{{ formatMessage(messages.retry) }}
							</Button>
						</template>
					</Admonition>

					<Admonition
						v-if="!unlimited && memory > budgetLeft && (ceiling ?? 0) >= MEMORY_MIN"
						type="warning"
						:body="formatMessage(messages.overBudget)"
					/>
				</Card>

				<Card v-if="worldSettingsShown" class="!mb-0 flex flex-col gap-5">
					<div class="flex flex-col gap-2">
						<span class="text-sm font-semibold text-contrast">
							{{ formatMessage(messages.gamemodeLabel) }}
						</span>
						<Chips
							v-model="gamemode"
							:items="gamemodes"
							:format-label="gamemodeLabel"
							:aria-label="formatMessage(messages.gamemodeLabel)"
						/>
					</div>

					<div class="flex flex-col gap-2">
						<span class="text-sm font-semibold text-contrast">
							{{ formatMessage(messages.difficultyLabel) }}
						</span>
						<Chips
							v-model="difficulty"
							:items="difficulties"
							:format-label="difficultyLabel"
							:aria-label="formatMessage(messages.difficultyLabel)"
						/>
					</div>

					<div class="flex flex-col gap-1.5">
						<label class="text-sm font-semibold text-contrast" for="world-seed">
							{{ formatMessage(messages.seedLabel) }}
						</label>
						<StyledInput
							id="world-seed"
							v-model="seed"
							:placeholder="formatMessage(messages.seedPlaceholder)"
							:disabled="busy"
							wrapper-class="w-full"
						/>
					</div>
				</Card>

				<Card class="!mb-0 flex flex-col gap-4">
					<div class="flex flex-col gap-1.5">
						<Checkbox
							v-model="eula"
							:disabled="busy"
							:label="formatMessage(messages.eulaLabel)"
							label-class="text-sm text-contrast"
						/>
						<a
							href="https://aka.ms/MinecraftEULA"
							target="_blank"
							rel="noopener noreferrer"
							class="ml-8 w-fit text-sm text-brand"
						>
							{{ formatMessage(messages.eulaLink) }}
						</a>
					</div>

					<Admonition v-if="submitFailure" type="critical" :body="submitFailure" />

					<div class="flex flex-wrap justify-end gap-2">
						<ButtonLink :to="{ name: 'servers' }" :disabled="busy">
							{{ formatMessage(commonMessages.cancelButton) }}
						</ButtonLink>
						<Button
							native-type="submit"
							type="colored"
							color="brand"
							size="lg"
							:disabled="!submittable"
							:loading="busy"
						>
							<SpinnerIcon v-if="busy" class="animate-spin" />
							<ServerPlusIcon v-else aria-hidden="true" />
							{{ formatMessage(messages.createButton) }}
						</Button>
					</div>
				</Card>
			</form>
		</template>

		<Card v-else class="!mb-0 flex flex-col gap-5">
			<div class="flex flex-col gap-1">
				<h2 class="m-0 text-lg font-semibold text-contrast">{{ created?.name }}</h2>
				<p class="m-0 text-secondary">{{ resultDescription }}</p>
			</div>

			<Admonition
				v-for="warning in warnings"
				:key="warning"
				type="warning"
				:body="formatMessage(warningMessages[warning])"
			/>

			<template v-if="stage === 'working'">
				<ProgressBar
					full-width
					show-progress
					:progress="operation?.progress ?? 0"
					:waiting="operation?.state === 'queued'"
					:label="formatMessage(phaseLabel)"
					label-class="text-sm font-medium text-secondary"
				/>
				<p v-if="operation?.message" class="m-0 truncate text-sm text-secondary">
					{{ operation.message }}
				</p>
			</template>

			<Admonition
				v-else-if="stage === 'failed'"
				type="critical"
				:header="formatMessage(messages.installFailed)"
				:body="operation?.error?.message"
			/>

			<Admonition v-if="submitFailure" type="critical" :body="submitFailure" />

			<div class="flex flex-wrap justify-end gap-2">
				<Button v-if="cancellable" :disabled="cancelling" @click="cancel">
					<XIcon aria-hidden="true" />
					{{ formatMessage(commonMessages.cancelButton) }}
				</Button>
				<Button v-if="stage === 'failed'" :disabled="retrying" @click="retry">
					<RotateCounterClockwiseIcon aria-hidden="true" />
					{{ formatMessage(messages.retry) }}
				</Button>
				<ButtonLink :to="{ name: 'servers' }">
					{{ formatMessage(commonMessages.serversLabel) }}
				</ButtonLink>
				<ButtonLink
					v-if="created"
					:to="{ name: 'server-overview', params: { id: created.id } }"
					type="colored"
					color="brand"
				>
					{{ formatMessage(messages.openServer) }}
					<RightArrowIcon aria-hidden="true" />
				</ButtonLink>
			</div>
		</Card>
	</div>
</template>

<script setup lang="ts">
import {
	EyeIcon,
	EyeOffIcon,
	LeftArrowIcon,
	RightArrowIcon,
	RotateCounterClockwiseIcon,
	ServerPlusIcon,
	SpinnerIcon,
	XIcon,
} from '@modrinth/assets'
import {
	Admonition,
	Button,
	ButtonLink,
	Card,
	Checkbox,
	Chips,
	Combobox,
	type ComboboxOption,
	commonMessages,
	defineMessages,
	LoadingIndicator,
	type MessageDescriptor,
	ProgressBar,
	Slider,
	StyledInput,
	useFormatBytes,
	useVIntl,
} from '@modrinth/ui'
import PaperChannelBadge from '@modrinth/ui/src/components/base/PaperChannelBadge.vue'
import { computed, onBeforeUnmount, onMounted, ref, watch } from 'vue'
import { useRouter } from 'vue-router'

import {
	api,
	type GameVersionEntry,
	isApiRequestError,
	type KnownProperties,
	type LoaderBuild,
	type LoaderId,
	type LoaderInfo,
	type Operation,
	type OperationPhase,
	openServerSocket,
	type Server,
	type ServerSocket,
	type ServerWarning,
	type Ulid,
} from '@/api'
import { useSession } from '@/composables/session'

import { type HostState, memoryCeiling } from './memory-ceiling'

const MEMORY_MIN = 512
const MEMORY_STEP = 256
const MEMORY_DEFAULT = 2048
const POLL_INTERVAL_MS = 5_000
const SUPPORTED_WAVE = 1

type Gamemode = 'survival' | 'creative' | 'hardcore'
type Difficulty = 'peaceful' | 'easy' | 'normal' | 'hard'

const gamemodes: Gamemode[] = ['survival', 'creative', 'hardcore']
const difficulties: Difficulty[] = ['peaceful', 'easy', 'normal', 'hard']

const { formatMessage } = useVIntl()
const formatBytes = useFormatBytes()
const router = useRouter()
const { user, isAdmin, refresh } = useSession()

const messages = defineMessages({
	title: {
		id: 'panel.server-new.title',
		defaultMessage: 'New server',
	},
	nameLabel: {
		id: 'panel.server-new.name.label',
		defaultMessage: 'Name',
	},
	namePlaceholder: {
		id: 'panel.server-new.name.placeholder',
		defaultMessage: 'My server',
	},
	buildLabel: {
		id: 'panel.server-new.build.label',
		defaultMessage: 'Build',
	},
	latestStable: {
		id: 'panel.server-new.build.latest-stable',
		defaultMessage: 'Latest stable build',
	},
	buildsTruncated: {
		id: 'panel.server-new.build.truncated',
		defaultMessage: 'Only the 500 newest builds are shown.',
	},
	noVersions: {
		id: 'panel.server-new.no-versions',
		defaultMessage: 'No versions available',
	},
	noBuilds: {
		id: 'panel.server-new.no-builds',
		defaultMessage: 'No builds available',
	},
	loaderNotWired: {
		id: 'panel.server-new.loader-not-wired',
		defaultMessage: 'This loader needs an installer step and is not available yet.',
	},
	memoryLabel: {
		id: 'panel.server-new.memory.label',
		defaultMessage: 'Memory',
	},
	memoryBudget: {
		id: 'panel.server-new.memory.budget',
		defaultMessage: '{free} left in your budget',
	},
	noMemoryLeft: {
		id: 'panel.server-new.memory.none-left',
		defaultMessage: 'Your budget has no room for another server. Free some memory first.',
	},
	hostUnavailable: {
		id: 'panel.server-new.memory.host-unavailable',
		defaultMessage:
			"The machine's memory could not be read, so the slider is not measured against it. Creating a server still works. Pick a value the machine can carry.",
	},
	overBudget: {
		id: 'panel.server-new.memory.over-budget',
		defaultMessage:
			'This goes past your own budget and into the machine. It will be allowed, but the memory is then overcommitted.',
	},
	gamemodeLabel: {
		id: 'panel.server-new.gamemode.label',
		defaultMessage: 'Game mode',
	},
	difficultyLabel: {
		id: 'panel.server-new.difficulty.label',
		defaultMessage: 'Difficulty',
	},
	seedLabel: {
		id: 'panel.server-new.seed.label',
		defaultMessage: 'World seed',
	},
	seedPlaceholder: {
		id: 'panel.server-new.seed.placeholder',
		defaultMessage: 'Leave empty for a random world',
	},
	eulaLabel: {
		id: 'panel.server-new.eula.label',
		defaultMessage: "I accept Minecraft's End User Licence Agreement.",
	},
	eulaLink: {
		id: 'panel.server-new.eula.link',
		defaultMessage: 'Read the agreement',
	},
	createButton: {
		id: 'panel.server-new.create',
		defaultMessage: 'Create server',
	},
	openServer: {
		id: 'panel.server-new.open-server',
		defaultMessage: 'Open server',
	},
	retry: {
		id: 'panel.server-new.retry',
		defaultMessage: 'Try again',
	},
	catalogFailed: {
		id: 'panel.server-new.catalog-failed',
		defaultMessage: 'The loader catalogue could not be loaded',
	},
	installFailed: {
		id: 'panel.server-new.install-failed',
		defaultMessage: 'Setting the server up failed',
	},
	working: {
		id: 'panel.server-new.working',
		defaultMessage: 'Your server is being set up. This usually takes a minute or two.',
	},
	cancelled: {
		id: 'panel.server-new.cancelled',
		defaultMessage:
			'Setup was cancelled. The server exists but has nothing on it yet. Set it up again or delete it.',
	},
	failedDescription: {
		id: 'panel.server-new.failed-description',
		defaultMessage: 'The server exists but has nothing on it yet.',
	},
	blockedHeader: {
		id: 'panel.server-new.blocked.header',
		defaultMessage: 'You cannot create a server right now',
	},
	overLimit: {
		id: 'panel.server-new.blocked.over-limit',
		defaultMessage:
			'Your servers already claim your whole memory budget. Free some up before making another one.',
	},
	systemUserNotReady: {
		id: 'panel.server-new.blocked.system-user',
		defaultMessage:
			'Your system account is not ready yet. Nothing can run until an administrator has fixed it.',
	},
	unreachable: {
		id: 'panel.server-new.error.unreachable',
		defaultMessage: 'The panel could not be reached.',
	},
	survival: { id: 'panel.server-new.gamemode.survival', defaultMessage: 'Survival' },
	creative: { id: 'panel.server-new.gamemode.creative', defaultMessage: 'Creative' },
	hardcore: { id: 'panel.server-new.gamemode.hardcore', defaultMessage: 'Hardcore' },
	peaceful: { id: 'panel.server-new.difficulty.peaceful', defaultMessage: 'Peaceful' },
	easy: { id: 'panel.server-new.difficulty.easy', defaultMessage: 'Easy' },
	normal: { id: 'panel.server-new.difficulty.normal', defaultMessage: 'Normal' },
	hard: { id: 'panel.server-new.difficulty.hard', defaultMessage: 'Hard' },
})

const submitFailures: Record<string, MessageDescriptor> = defineMessages({
	invalid_request: {
		id: 'panel.server-new.error.invalid-request',
		defaultMessage: 'The name may not be empty or longer than 64 characters.',
	},
	budget_exceeded: {
		id: 'panel.server-new.error.budget-exceeded',
		defaultMessage: 'That is more memory than your budget has left.',
	},
	over_limit: {
		id: 'panel.server-new.error.over-limit',
		defaultMessage: 'Your servers already claim more memory than your budget allows.',
	},
	port_pool_exhausted: {
		id: 'panel.server-new.error.port-pool-exhausted',
		defaultMessage: 'No free port is left in the pool. An administrator has to widen it.',
	},
	port_in_use: {
		id: 'panel.server-new.error.port-in-use',
		defaultMessage: 'The port is taken. Try again.',
	},
	unsupported_game_version: {
		id: 'panel.server-new.error.unsupported-game-version',
		defaultMessage: 'This loader does not offer that game version.',
	},
	unknown_loader: {
		id: 'panel.server-new.error.unknown-loader',
		defaultMessage: 'This loader is unknown to the panel.',
	},
	upstream_unavailable: {
		id: 'panel.server-new.error.upstream-unavailable',
		defaultMessage: 'The download source could not be reached. Try again in a moment.',
	},
	network_unreachable: {
		id: 'panel.server-new.error.network-unreachable',
		defaultMessage: 'The panel could not be reached.',
	},
})

const warningMessages: Record<ServerWarning, MessageDescriptor> = defineMessages({
	memory_overcommitted: {
		id: 'panel.server-new.warning.memory-overcommitted',
		defaultMessage: 'The machine now promises more memory than it has.',
	},
	properties_will_be_ignored: {
		id: 'panel.server-new.warning.properties-ignored',
		defaultMessage: 'This loader has no server.properties, so the world settings were dropped.',
	},
})

const phaseMessages: Record<OperationPhase, MessageDescriptor> = defineMessages({
	analyzing: { id: 'panel.phase.analyzing', defaultMessage: 'Resolving the version' },
	installing_loader: { id: 'panel.phase.installing-loader', defaultMessage: 'Downloading' },
	verifying: { id: 'panel.phase.verifying', defaultMessage: 'Checking the download' },
	running_installer: { id: 'panel.phase.running-installer', defaultMessage: 'Running the installer' },
	installing_pack: { id: 'panel.phase.installing-pack', defaultMessage: 'Unpacking the modpack' },
	addons: { id: 'panel.phase.addons', defaultMessage: 'Fetching content' },
	writing_config: { id: 'panel.phase.writing-config', defaultMessage: 'Writing the configuration' },
})

const loaders = ref<LoaderInfo[]>([])
const catalogLoading = ref(true)
const catalogFailure = ref<string | null>(null)

const name = ref('')
const loader = ref<LoaderId | null>(null)
const gameVersion = ref<string | null>(null)
const build = ref('')
const showSnapshots = ref(false)
const memory = ref(MEMORY_DEFAULT)
const eula = ref(false)
const gamemode = ref<Gamemode>('survival')
const difficulty = ref<Difficulty>('normal')
const seed = ref('')

const versions = ref<GameVersionEntry[]>([])
const versionsLoading = ref(false)
const versionsFailure = ref<string | null>(null)

const builds = ref<LoaderBuild[]>([])
const buildsLoading = ref(false)
const buildsFailure = ref<string | null>(null)
const buildsTruncated = ref(false)

const stage = ref<'form' | 'working' | 'failed' | 'cancelled'>('form')
const busy = ref(false)
const submitFailure = ref<string | null>(null)
const created = ref<Server | null>(null)
const operation = ref<Operation | null>(null)
const warnings = ref<ServerWarning[]>([])
const cancelling = ref(false)
const retrying = ref(false)
const hostState = ref<HostState>('loading')
const hostAssignable = ref<number | null>(null)

let socket: ServerSocket | null = null
let poller: ReturnType<typeof setInterval> | null = null
let versionToken = 0
let buildToken = 0

const blocked = computed(() => {
	switch (user.value?.capabilities.blocked_reason) {
		case 'over_limit':
			return formatMessage(messages.overLimit)
		case 'system_user_not_ready':
			return formatMessage(messages.systemUserNotReady)
		default:
			return null
	}
})

const loaderIds = computed(() => loaders.value.map((entry) => entry.id))
const unwiredLoaders = computed(() =>
	loaders.value.filter((entry) => entry.wave > SUPPORTED_WAVE).map((entry) => entry.id),
)
const selectedLoader = computed(() =>
	loaders.value.find((entry) => entry.id === loader.value) ?? null,
)
const worldSettingsShown = computed(() => selectedLoader.value?.supports_properties !== false)

function loaderLabel(id: LoaderId): string {
	return loaders.value.find((entry) => entry.id === id)?.name ?? id
}

function gamemodeLabel(value: Gamemode): string {
	return formatMessage(messages[value])
}

function difficultyLabel(value: Difficulty): string {
	return formatMessage(messages[value])
}

const versionOptions = computed<ComboboxOption<string>[]>(() =>
	versions.value
		.filter((entry) => showSnapshots.value || entry.version_type === 'release')
		.map((entry) => ({ value: entry.version, label: entry.version })),
)

const buildOptions = computed<ComboboxOption<string>[]>(() => [
	{ value: '', label: formatMessage(messages.latestStable) },
	...builds.value.map((entry) => ({ value: entry.id, label: entry.label })),
])

function channelOf(id: string): 'ALPHA' | 'BETA' | null {
	return builds.value.find((entry) => entry.id === id)?.channel_tag ?? null
}

const channelled = computed(() => builds.value.some((entry) => entry.channel_tag !== null))

const unlimited = computed(() => user.value?.usage.memory.limit_mib === null)

const budgetLeft = computed(() => {
	const usage = user.value?.usage.memory
	if (!usage || usage.limit_mib === null) return 0
	return Math.max(0, usage.limit_mib - usage.allocated_mib)
})

const capacity = computed(() =>
	memoryCeiling(user.value?.usage.memory ?? { limit_mib: 0, allocated_mib: 0, used_bytes: 0 }, {
		state: hostState.value,
		assignableMib: hostAssignable.value,
	}),
)

const ceiling = computed<number | null>(() => capacity.value.max)

const submittable = computed(
	() =>
		!busy.value &&
		name.value.trim().length > 0 &&
		loader.value !== null &&
		gameVersion.value !== null &&
		eula.value &&
		ceiling.value !== null &&
		ceiling.value >= MEMORY_MIN,
)

const cancellable = computed(() => stage.value === 'working' && operation.value?.cancellable === true)

const phaseLabel = computed(() =>
	operation.value?.phase ? phaseMessages[operation.value.phase] : messages.working,
)

const resultDescription = computed(() => {
	if (stage.value === 'cancelled') return formatMessage(messages.cancelled)
	if (stage.value === 'failed') return formatMessage(messages.failedDescription)
	return formatMessage(messages.working)
})

function reason(error: unknown, table: Record<string, MessageDescriptor> = {}): string {
	if (!isApiRequestError(error)) return formatMessage(messages.unreachable)
	const known = table[error.code]
	return known ? formatMessage(known) : error.message || formatMessage(messages.unreachable)
}

async function loadCatalog(): Promise<void> {
	catalogLoading.value = true
	catalogFailure.value = null
	try {
		const catalog = await api.settings.loaders()
		loaders.value = catalog.loaders
		loader.value ??= catalog.loaders.find((entry) => entry.wave <= SUPPORTED_WAVE)?.id ?? null
	} catch (error) {
		catalogFailure.value = reason(error)
	} finally {
		catalogLoading.value = false
	}
}

async function loadVersions(id: LoaderId): Promise<void> {
	const token = ++versionToken
	versionsLoading.value = true
	versionsFailure.value = null
	versions.value = []
	try {
		const list = await api.settings.gameVersions(id)
		if (token !== versionToken) return
		versions.value = list.game_versions
	} catch (error) {
		if (token !== versionToken) return
		versionsFailure.value = reason(error, submitFailures)
	} finally {
		if (token === versionToken) versionsLoading.value = false
	}
}

async function loadBuilds(id: LoaderId, version: string): Promise<void> {
	const token = ++buildToken
	buildsLoading.value = true
	buildsFailure.value = null
	builds.value = []
	buildsTruncated.value = false
	try {
		const list = await api.settings.builds(id, version)
		if (token !== buildToken) return
		builds.value = list.builds
		buildsTruncated.value = list.truncated
	} catch (error) {
		if (token !== buildToken) return
		buildsFailure.value = reason(error, submitFailures)
	} finally {
		if (token === buildToken) buildsLoading.value = false
	}
}

watch([versionOptions, versionsLoading], ([options, loading]) => {
	if (loading) return
	if (options.length === 0) {
		gameVersion.value = null
		return
	}
	if (!gameVersion.value || !options.some((option) => option.value === gameVersion.value)) {
		gameVersion.value = options[0].value
	}
})

watch(loader, (id) => {
	gameVersion.value = null
	build.value = ''
	builds.value = []
	if (id) void loadVersions(id)
})

watch([loader, gameVersion], ([id, version]) => {
	build.value = ''
	if (!id || !version || !selectedLoader.value?.has_loader_versions) {
		builds.value = []
		return
	}
	void loadBuilds(id, version)
})

watch(ceiling, (limit) => {
	if (limit === null || limit < MEMORY_MIN) return
	memory.value = Math.min(Math.max(memory.value, MEMORY_MIN), limit)
})

async function create(): Promise<void> {
	if (!submittable.value || loader.value === null || gameVersion.value === null) return
	busy.value = true
	submitFailure.value = null

	try {
		const response = await api.servers.create({
			name: name.value.trim(),
			owner_id: null,
			memory_mib: memory.value,
			port: null,
			eula_accepted: true,
			content: {
				kind: 'loader',
				loader: loader.value,
				game_version: gameVersion.value,
				loader_version: build.value === '' ? null : build.value,
			},
			properties: { known: properties() },
		})
		created.value = response.server
		warnings.value = response.warnings ?? []
		stage.value = 'working'
		follow(response.server.id, response.operation.id)
		apply(response.operation)
		void refresh()
	} catch (error) {
		submitFailure.value = reason(error, submitFailures)
	} finally {
		busy.value = false
	}
}

function properties(): KnownProperties {
	if (!worldSettingsShown.value) return {}
	const isHardcore = gamemode.value === 'hardcore'
	return {
		gamemode: isHardcore ? 'survival' : gamemode.value,
		hardcore: isHardcore ? 'true' : 'false',
		difficulty: difficulty.value,
		level_seed: seed.value.trim() || null,
	}
}

function follow(serverId: Ulid, operationId: Ulid): void {
	unfollow()
	socket = openServerSocket(serverId)
	socket.on('operations', (message) => {
		const found = message.operations.find((entry) => entry.id === operationId)
		if (found) apply(found)
	})
	poller = setInterval(() => {
		if (socket?.status.phase === 'open') return
		api.operations.get(serverId, operationId).then(apply, () => undefined)
	}, POLL_INTERVAL_MS)
}

function unfollow(): void {
	socket?.close()
	socket = null
	if (poller !== null) clearInterval(poller)
	poller = null
}

function apply(next: Operation): void {
	operation.value = next
	if (next.state === 'done') {
		unfollow()
		void refresh()
		void router.replace({ name: 'server-overview', params: { id: next.server_id } })
		return
	}
	if (next.state === 'failed') {
		unfollow()
		stage.value = 'failed'
		return
	}
	if (next.state === 'cancelled') {
		unfollow()
		stage.value = 'cancelled'
		void refresh()
	}
}

async function cancel(): Promise<void> {
	if (!created.value || !operation.value || cancelling.value) return
	cancelling.value = true
	try {
		apply(await api.operations.cancel(created.value.id, operation.value.id))
	} catch (error) {
		submitFailure.value = reason(error)
	} finally {
		cancelling.value = false
	}
}

async function retry(): Promise<void> {
	if (!created.value || !operation.value || retrying.value) return
	retrying.value = true
	try {
		const accepted = await api.operations.retry(created.value.id, operation.value.id)
		stage.value = 'working'
		submitFailure.value = null
		follow(created.value.id, accepted.operation.id)
		apply(accepted.operation)
	} catch (error) {
		submitFailure.value = reason(error)
	} finally {
		retrying.value = false
	}
}

async function loadHost(): Promise<void> {
	hostState.value = 'loading'
	try {
		hostAssignable.value = (await api.admin.host()).assignable_memory_mib
		hostState.value = 'ready'
	} catch {
		hostAssignable.value = null
		hostState.value = 'unavailable'
	}
}

onMounted(() => {
	memory.value = unlimited.value
		? MEMORY_DEFAULT
		: Math.min(MEMORY_DEFAULT, Math.max(MEMORY_MIN, budgetLeft.value))
	void loadCatalog()
	if (isAdmin.value) void loadHost()
	else hostState.value = 'unavailable'
})

onBeforeUnmount(unfollow)
</script>
