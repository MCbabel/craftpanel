<template>
	<div class="relative flex flex-col gap-6">
		<LoadingIndicator v-if="loading" />
		<ErrorInformationCard
			v-else-if="error"
			:title="formatMessage(messages.loadFailed)"
			:description="error"
			:icon="IssuesIcon"
			:action="{ label: formatMessage(commonMessages.retryButton), onClick: () => void load() }"
		/>

		<template v-else-if="startup">
			<Admonition
				v-if="isAdmin && startup.stripped_flags.length > 0"
				type="warning"
				:header="formatMessage(messages.strippedHeader)"
				:body="formatMessage(messages.strippedBody, { flags: startup.stripped_flags.join(' ') })"
			/>

			<div class="flex flex-col gap-2.5">
				<span class="text-lg font-semibold text-contrast">
					{{ formatMessage(messages.memory) }}
				</span>
				<Admonition
					v-if="maxGb < MEMORY_MIN_GB"
					type="warning"
					:body="formatMessage(messages.noMemoryLeft)"
				/>
				<template v-else>
					<Slider
						v-model="memoryGb"
						:min="MEMORY_MIN_GB"
						:max="maxGb"
						:step="1"
						:disabled="!canUseAdvancedSettings"
						unit="GB"
					/>
					<span>{{ formatMessage(messages.memoryHelp) }}</span>
				</template>
			</div>

			<template v-if="isAdmin">
				<div class="flex flex-col gap-2.5">
					<div class="flex h-10 flex-col items-end justify-between gap-4 sm:flex-row">
						<label for="startup-command-field" class="mb-0.5 text-lg font-semibold text-contrast">
							{{ formatMessage(messages.command) }}
						</label>
						<Button
							v-if="command !== startup.original_invocation"
							v-tooltip="advancedTooltip"
							type="quiet"
							:disabled="!canUseAdvancedSettings"
							@click="command = startup.original_invocation"
						>
							<UpdatedIcon class="size-5" />
							{{ formatMessage(messages.default) }}
						</Button>
					</div>
					<StyledInput
						id="startup-command-field"
						v-model="command"
						v-tooltip="advancedTooltip"
						multiline
						resize="vertical"
						input-class="font-mono"
						:disabled="!canUseAdvancedSettings"
					/>
					<span>{{ formatMessage(messages.commandHelp) }}</span>
				</div>

				<div v-if="startup.managed_flags.length > 0" class="flex flex-col gap-2.5">
					<span class="text-lg font-semibold text-contrast">
						{{ formatMessage(messages.managed) }}
					</span>
					<div class="flex flex-wrap gap-2 rounded-xl bg-surface-2 p-4 font-mono text-sm">
						<TagItem v-for="flag in startup.managed_flags" :key="flag" class="!font-medium">
							{{ flag }}
						</TagItem>
					</div>
					<span>{{ formatMessage(messages.managedHelp) }}</span>
				</div>
			</template>
			<span v-else>{{ formatMessage(messages.commandIsAdmins) }}</span>

			<div class="flex flex-col gap-2.5">
				<span class="text-lg font-semibold text-contrast">
					{{ formatMessage(messages.javaVersion) }}
				</span>
				<Chips
					v-model="javaVersion"
					:items="javaVersionItems"
					:format-label="javaVersionLabel"
					:disabled-items="canUseAdvancedSettings ? [] : javaVersionItems"
					:disabled-tooltip="permissionDeniedMessage"
					:capitalize="false"
					:aria-label="formatMessage(messages.javaVersion)"
				/>
				<span>{{ formatMessage(messages.javaVersionHelp) }}</span>
			</div>

			<div class="flex flex-col gap-2.5">
				<span class="text-lg font-semibold text-contrast">
					{{ formatMessage(messages.runtime) }}
				</span>
				<Chips
					v-model="vendor"
					:items="vendorItems"
					:format-label="vendorLabel"
					:disabled-items="canUseAdvancedSettings ? [] : vendorItems"
					:disabled-tooltip="permissionDeniedMessage"
					:capitalize="false"
					:aria-label="formatMessage(messages.runtime)"
				/>
				<span>{{ formatMessage(messages.runtimeHelp) }}</span>
			</div>

			<SaveBanner
				:is-visible="hasChanges || saving"
				:server-id="serverId"
				:is-updating="saving"
				:restart="canUsePowerActions"
				:save="save"
				:reset="syncFromServer"
			/>
		</template>
	</div>
</template>

<script setup lang="ts">
import { IssuesIcon, UpdatedIcon } from '@modrinth/assets'
import {
	Admonition,
	Button,
	Chips,
	commonMessages,
	defineMessages,
	ErrorInformationCard,
	injectModrinthServerContext,
	injectNotificationManager,
	LoadingIndicator,
	SaveBanner,
	Slider,
	StyledInput,
	TagItem,
	useServerPermissions,
	useVIntl,
} from '@modrinth/ui'
import { computed, onMounted, ref } from 'vue'

import { api, type JavaRuntime, type JreVendor, type StartupOptions } from '@/api'
import { useSession } from '@/composables/session'

import { gigabytes, mebibytes, MEMORY_MIN_GB, wholeGigabytes } from './memory-gb'

const VENDOR_LABELS: Record<JreVendor, string> = {
	corretto: 'Corretto',
	temurin: 'Temurin',
	graal: 'GraalVM',
}

const { formatMessage } = useVIntl()
const { addNotification } = injectNotificationManager()
const { serverId } = injectModrinthServerContext()
const { isAdmin } = useSession()
const { canUseAdvancedSettings, canUsePowerActions, permissionDeniedMessage } =
	useServerPermissions()

const messages = defineMessages({
	memory: { id: 'craftpanel.settings.advanced.memory', defaultMessage: 'Memory' },
	memoryHelp: {
		id: 'craftpanel.settings.advanced.memory-help',
		defaultMessage:
			'The slider ends at what this server may still be given: what is left of the owner’s memory budget, or what the machine can hand out if he has none.',
	},
	noMemoryLeft: {
		id: 'craftpanel.settings.advanced.no-memory-left',
		defaultMessage:
			'There is not a whole gigabyte left for this server — the other servers on this account hold the rest of the memory budget.',
	},
	command: { id: 'craftpanel.settings.advanced.command', defaultMessage: 'Startup command' },
	commandHelp: {
		id: 'craftpanel.settings.advanced.command-help',
		defaultMessage: 'The command that runs when your server starts.',
	},
	commandIsAdmins: {
		id: 'craftpanel.settings.advanced.command-is-admins',
		defaultMessage:
			'The startup command and its flags belong to the panel administrator. One flag Java does not know and the server stops starting, with nothing in the console to say why, so the panel keeps that field to itself and sets the memory from the slider above.',
	},
	default: { id: 'craftpanel.settings.advanced.default', defaultMessage: 'Default' },
	managed: { id: 'craftpanel.settings.advanced.managed', defaultMessage: 'Flags set by the panel' },
	managedHelp: {
		id: 'craftpanel.settings.advanced.managed-help',
		defaultMessage: 'These are added on every start and cannot be edited here.',
	},
	strippedHeader: {
		id: 'craftpanel.settings.advanced.stripped-header',
		defaultMessage: 'This save dropped some of what you typed',
	},
	strippedBody: {
		id: 'craftpanel.settings.advanced.stripped-body',
		defaultMessage:
			'The memory comes from the slider above, and beside it only Java flags are kept. Dropped: {flags}',
	},
	javaVersion: { id: 'craftpanel.settings.advanced.java-version', defaultMessage: 'Java version' },
	javaVersionHelp: {
		id: 'craftpanel.settings.advanced.java-version-help',
		defaultMessage: 'Only versions this machine can provide are offered.',
	},
	javaAutomatic: { id: 'craftpanel.settings.advanced.java-automatic', defaultMessage: 'Automatic' },
	javaNotInstalled: {
		id: 'craftpanel.settings.advanced.java-not-installed',
		defaultMessage: 'Java {major} (will be downloaded)',
	},
	javaInstalled: { id: 'craftpanel.settings.advanced.java-installed', defaultMessage: 'Java {major}' },
	runtime: { id: 'craftpanel.settings.advanced.runtime', defaultMessage: 'Java runtime' },
	runtimeHelp: {
		id: 'craftpanel.settings.advanced.runtime-help',
		defaultMessage: 'Which build of Java your server runs on.',
	},
	loadFailed: {
		id: 'craftpanel.settings.advanced.load-failed',
		defaultMessage: 'Failed to load the startup options',
	},
	saveFailed: {
		id: 'craftpanel.settings.advanced.save-failed',
		defaultMessage: 'Failed to update the startup options',
	},
	saved: { id: 'craftpanel.settings.advanced.saved', defaultMessage: 'Startup options updated' },
})

const startup = ref<StartupOptions | null>(null)
const runtimes = ref<JavaRuntime[]>([])
const loading = ref(true)
const saving = ref(false)
const error = ref<string | null>(null)

const command = ref('')
const memory = ref(mebibytes(MEMORY_MIN_GB))
const javaVersion = ref<number | null>(null)
const vendor = ref<JreVendor | null>(null)

const advancedTooltip = computed(() =>
	canUseAdvancedSettings.value ? undefined : permissionDeniedMessage.value,
)

const maxGb = computed(() => wholeGigabytes(startup.value?.memory_max_mib ?? 0))
const memoryGb = computed({
	get: () => gigabytes(memory.value),
	set: (value: number) => {
		memory.value = mebibytes(value)
	},
})

const javaVersionItems = computed<(number | null)[]>(() => [
	null,
	...new Set(runtimes.value.map((runtime) => runtime.major).sort((a, b) => a - b)),
])

const vendorItems = computed<(JreVendor | null)[]>(() => [
	null,
	...new Set(runtimes.value.map((runtime) => runtime.vendor)),
])

function javaVersionLabel(major: number | null): string {
	if (major === null) return formatMessage(messages.javaAutomatic)
	return runtimes.value.some((runtime) => runtime.major === major && runtime.installed)
		? formatMessage(messages.javaInstalled, { major })
		: formatMessage(messages.javaNotInstalled, { major })
}

function vendorLabel(value: JreVendor | null): string {
	return value === null ? formatMessage(messages.javaAutomatic) : VENDOR_LABELS[value]
}

const hasChanges = computed(() => {
	const saved = startup.value
	if (saved === null) return false
	return (
		command.value !== saved.startup_command ||
		memory.value !== saved.memory_mib ||
		javaVersion.value !== saved.java_version ||
		vendor.value !== saved.jre_vendor
	)
})

function syncFromServer(): void {
	const saved = startup.value
	if (saved === null) return
	command.value = saved.startup_command
	memory.value = saved.memory_mib
	javaVersion.value = saved.java_version
	vendor.value = saved.jre_vendor
}

async function load(): Promise<void> {
	loading.value = true
	error.value = null
	try {
		const [options, catalogue] = await Promise.all([
			api.settings.startup(serverId),
			api.settings.javaRuntimes({ server_id: serverId }),
		])
		startup.value = options
		runtimes.value = catalogue.runtimes
		syncFromServer()
	} catch (cause) {
		error.value = cause instanceof Error ? cause.message : String(cause)
	} finally {
		loading.value = false
	}
}

onMounted(() => void load())

async function save(): Promise<void> {
	if (!canUseAdvancedSettings.value || startup.value === null) return
	saving.value = true
	try {
		startup.value = await api.settings.patchStartup(serverId, {
			...(isAdmin.value ? { startup_command: command.value } : {}),
			memory_mib: memory.value,
			java_version: javaVersion.value,
			jre_vendor: vendor.value,
		})
		syncFromServer()
		addNotification({ type: 'success', title: formatMessage(messages.saved) })
	} catch (cause) {
		addNotification({
			type: 'error',
			title: formatMessage(messages.saveFailed),
			text: cause instanceof Error ? cause.message : undefined,
		})
	} finally {
		saving.value = false
	}
}
</script>
