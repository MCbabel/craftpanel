<template>
	<div class="flex flex-col gap-4">
		<div class="flex flex-wrap items-center justify-between gap-3">
			<div class="flex flex-col gap-1">
				<h1 class="m-0 text-2xl font-extrabold text-contrast">
					{{ formatMessage(messages.title) }}
				</h1>
				<span class="text-secondary">
					{{ formatMessage(messages.quota, { used: usedQuota, total: quota }) }}
				</span>
			</div>
			<Button
				v-tooltip="createTooltip"
				type="colored"
				color="brand"
				:disabled="createDisabled"
				@click="createModal?.show()"
			>
				<PlusIcon />
				{{ formatMessage(messages.create) }}
			</Button>
		</div>

		<Admonition
			v-if="query.isError.value"
			type="critical"
			:header="formatMessage(messages.loadFailed)"
			:body="errorText"
		/>

		<Admonition
			v-if="cannotBackUp"
			type="critical"
			:header="formatMessage(messages.driveBrokenHeader)"
			:body="formatMessage(driveBrokenBody)"
			show-actions-underneath
		>
			<template v-if="ownerCanConnect" #actions>
				<Button @click="router.push({ name: 'account' })">
					<SettingsIcon aria-hidden="true" />
					{{ formatMessage(messages.driveConnect) }}
				</Button>
			</template>
		</Admonition>

		<div
			v-if="target && target.policy !== 'local_only'"
			class="flex flex-wrap items-center justify-between gap-3 rounded-xl bg-surface-2 p-3"
		>
			<div class="flex flex-col">
				<span class="text-sm font-semibold text-contrast">
					{{ formatMessage(messages.targetTitle) }}
				</span>
				<span class="text-sm text-secondary">
					{{
						formatMessage(
							target.effective_target === 'drive' ? messages.targetDrive : messages.targetLocal,
						)
					}}
					{{ target.reason === 'ok' ? '' : formatMessage(TARGET_REASONS[target.reason]) }}
				</span>
			</div>
			<Toggle
				v-if="targetIsChoosable(target)"
				:model-value="target.target === 'drive'"
				:disabled="!canManageBackups || savingTarget"
				@update:model-value="switchTarget"
			/>
		</div>

		<div class="flex flex-wrap items-center justify-between gap-3">
			<Chips v-model="filter" :items="filters" :format-label="filterLabel" />
			<div v-if="selected.size > 0" class="flex items-center gap-2">
				<span class="text-secondary">
					{{ formatMessage(messages.selected, { count: selected.size }) }}
				</span>
				<Button @click="selected.clear()">
					<XIcon />
					{{ formatMessage(messages.clearSelection) }}
				</Button>
				<Button
					v-tooltip="backupsTooltip"
					type="colored"
					color="red"
					:disabled="!canManageBackups"
					@click="deleteModal?.showBulk(selectedBackups)"
				>
					<TrashIcon />
					{{ formatMessage(commonMessages.deleteLabel) }}
				</Button>
			</div>
		</div>

		<LoadingIndicator v-if="query.isPending.value" />
		<EmptyState
			v-else-if="visibleBackups.length === 0 && !query.isError.value"
			:heading="formatMessage(messages.emptyHeading)"
			:description="formatMessage(messages.emptyDescription)"
		/>

		<div v-else class="flex flex-col gap-3">
			<div v-for="backup in visibleBackups" :key="backup.id" class="flex flex-col gap-1.5">
				<div class="flex items-center gap-3">
					<Checkbox
						:model-value="selected.has(backup.id)"
						:disabled="!canManageBackups"
						@update:model-value="toggle(backup.id)"
					/>
					<BackupItem
						class="min-w-0 flex-1"
						:backup="backup"
						:creator="null"
						:selected="selected.has(backup.id)"
						:restore-disabled="restoreDisabled(backup)"
						:write-disabled="!canManageBackups"
						:write-disabled-tooltip="permissionDeniedMessage"
						:kyros-url="downloadHost"
						:jwt="downloadHost ? 'cookie' : undefined"
						show-copy-id-action
						@rename="renameModal?.show(backup)"
						@restore="restoreModal?.show(backup)"
						@delete="deleteModal?.show(backup)"
					/>
					<IconButton
						v-if="openableInDrive(factsOf(backup))"
						type="quiet"
						:label="formatMessage(messages.openInDrive)"
						@click="openInDrive(backup)"
					>
						<ExternalIcon />
					</IconButton>
					<IconButton
						v-else-if="downloadHost === undefined"
						type="quiet"
						:label="formatMessage(commonMessages.downloadButton)"
						@click="download(backup)"
					>
						<DownloadIcon />
					</IconButton>
				</div>
				<div
					v-if="factsOf(backup).location === 'drive'"
					class="ml-9 flex flex-wrap items-center gap-2 text-sm"
				>
					<Badge
						:type="formatMessage(FILE_STATES[factsOf(backup).state ?? 'present'])"
						:color="FILE_COLORS[factsOf(backup).state ?? 'present']"
					/>
					<span v-if="notRestorable(factsOf(backup))" class="text-secondary">
						{{ formatMessage(messages.driveGoneHint) }}
					</span>
				</div>

				<div v-if="runningOf(backup)" class="ml-9 flex flex-col gap-1">
					<span class="text-sm font-medium text-secondary">
						{{
							formatMessage(
								runningOf(backup) === 'restore' ? messages.restoring : messages.creating,
							)
						}}
					</span>
					<ProgressBar
						:progress="progressOf(backup)"
						:waiting="progressOf(backup) === 0"
						full-width
					/>
				</div>
				<Admonition
					v-else-if="failureOf(backup)"
					class="ml-9"
					type="critical"
					:header="formatMessage(messages.failed)"
					:body="failureBody(backup)"
					show-actions-underneath
				>
					<template #actions>
						<Button
							v-tooltip="retryTooltip"
							:disabled="retryDisabled"
							@click="retry(backup.id)"
						>
							<UpdatedIcon />
							{{ formatMessage(commonMessages.retryButton) }}
						</Button>
					</template>
				</Admonition>
			</div>
		</div>

		<div class="mt-2 flex flex-col gap-3 rounded-2xl bg-surface-2 p-4">
			<div class="flex items-center justify-between gap-4">
				<div class="flex flex-col gap-1">
					<span class="text-lg font-semibold text-contrast">
						{{ formatMessage(messages.schedule) }}
					</span>
					<span>{{ formatMessage(messages.scheduleHelp) }}</span>
					<span v-if="cannotBackUp" class="text-sm text-orange">
						{{ formatMessage(messages.scheduleImpossible) }}
					</span>
				</div>
				<Toggle
					id="backup-schedule-enabled"
					:model-value="schedule?.enabled ?? false"
					:disabled="scheduleLocked"
					@update:model-value="(value?: boolean) => saveSchedule({ enabled: value === true })"
				/>
			</div>

			<div v-if="schedule?.enabled" class="flex flex-wrap items-end gap-4">
				<label class="flex flex-col gap-1.5">
					<span class="font-semibold text-contrast">{{ formatMessage(messages.every) }}</span>
					<StyledInput
						:model-value="String(schedule.interval_hours)"
						type="number"
						:min="1"
						:max="168"
						wrapper-class="w-[8rem]"
						:disabled="!canManageBackups"
						@update:model-value="
								(value?: string | number) =>
								within(value, 1, 168, (hours) => ({ interval_hours: hours }))
						"
					/>
				</label>
				<label v-if="schedule.interval_hours % 24 === 0" class="flex flex-col gap-1.5">
					<span class="font-semibold text-contrast">{{ formatMessage(messages.atHour) }}</span>
					<StyledInput
						:model-value="String(schedule.hour_utc)"
						type="number"
						:min="0"
						:max="23"
						wrapper-class="w-[8rem]"
						:disabled="!canManageBackups"
						@update:model-value="
							(value?: string | number) => within(value, 0, 23, (hour) => ({ hour_utc: hour }))
						"
					/>
				</label>
				<label class="flex flex-col gap-1.5">
					<span class="font-semibold text-contrast">{{ formatMessage(messages.keepLast) }}</span>
					<StyledInput
						:model-value="String(schedule.keep_last)"
						type="number"
						:min="1"
						:max="keepLastMax"
						wrapper-class="w-[8rem]"
						:disabled="!canManageBackups"
						@update:model-value="
								(value?: string | number) =>
								within(value, 1, keepLastMax, (keep) => ({ keep_last: keep }))
						"
					/>
				</label>
				<span v-if="schedule.next_run_at" class="pb-2 text-secondary">
					{{ formatMessage(messages.nextRun, { time: formatDateTime(schedule.next_run_at) }) }}
				</span>
			</div>
		</div>

		<Teleport to="body">
			<div class="relative z-[100]">
				<BackupCreateModal
					ref="createModal"
					:backups="backups"
					:can-create="!createDisabled"
					:permission-denied-message="permissionDeniedMessage"
				/>
				<BackupRenameModal
					ref="renameModal"
					:backups="backups"
					:can-rename="canManageBackups"
					:permission-denied-message="permissionDeniedMessage"
				/>
				<BackupRestoreModal
					ref="restoreModal"
					:can-restore="canManageBackups"
					:permission-denied-message="permissionDeniedMessage"
				/>
				<BackupDeleteModal
					ref="deleteModal"
					:can-delete="canManageBackups"
					:permission-denied-message="permissionDeniedMessage"
					@delete="removeOne"
					@bulk-delete="removeMany"
				/>
			</div>
		</Teleport>
	</div>
</template>

<script setup lang="ts">
import type { Archon } from '@modrinth/api-client'
import {
	DownloadIcon,
	ExternalIcon,
	PlusIcon,
	SettingsIcon,
	TrashIcon,
	UpdatedIcon,
	XIcon,
} from '@modrinth/assets'
import {
	Admonition,
	BackupCreateModal,
	BackupDeleteModal,
	BackupItem,
	BackupRenameModal,
	BackupRestoreModal,
	Badge,
	Button,
	Checkbox,
	Chips,
	commonMessages,
	defineMessages,
	EmptyState,
	IconButton,
	injectModrinthServerContext,
	injectNotificationManager,
	LoadingIndicator,
	ProgressBar,
	StyledInput,
	Toggle,
	useFormatDateTime,
	useServerBackupsQueue,
	useServerPermissions,
	useVIntl,
} from '@modrinth/ui'
import { computed, reactive, ref, watch } from 'vue'
import { useRouter } from 'vue-router'

import {
	api,
	type BackupSchedule,
	type OperationKind,
	type UpdateBackupScheduleRequest,
} from '@/api'
import { type BackupTarget, drive } from '@/api/drive'
import { useServerPage } from '@/composables/server-page'
import { useSession } from '@/composables/session'

import {
	backupImpossible,
	driveFactsOf,
	needsOwnersDrive,
	notRestorable,
	openableInDrive,
	targetIsChoosable,
} from './backup-target'

type QueueBackup = Archon.BackupsQueue.v1.BackupQueueBackup

const MAX_BACKUP_QUOTA = 50

const BACKUP_KINDS = new Set<OperationKind>(['backup_create', 'backup_restore'])

const { formatMessage } = useVIntl()
const formatDateTime = useFormatDateTime({ dateStyle: 'medium', timeStyle: 'short' })
const router = useRouter()
const { serverId, server, worldId } = injectModrinthServerContext()
const { addNotification } = injectNotificationManager()
const { canManageBackups, permissionDeniedMessage } = useServerPermissions()
const { user } = useSession()
const { operations } = useServerPage()

const messages = defineMessages({
	title: { id: 'craftpanel.backups.title', defaultMessage: 'Backups' },
	quota: { id: 'craftpanel.backups.quota', defaultMessage: '{used} of {total} used' },
	create: { id: 'craftpanel.backups.create', defaultMessage: 'Create backup' },
	quotaFull: { id: 'craftpanel.backups.quota-full', defaultMessage: 'No backup slots left.' },
	busy: { id: 'craftpanel.backups.busy', defaultMessage: 'A backup is already running.' },
	all: { id: 'craftpanel.backups.filter-all', defaultMessage: 'All' },
	manual: { id: 'craftpanel.backups.filter-manual', defaultMessage: 'Manual' },
	auto: { id: 'craftpanel.backups.filter-auto', defaultMessage: 'Auto' },
	selected: { id: 'craftpanel.backups.selected', defaultMessage: '{count} selected' },
	clearSelection: { id: 'craftpanel.backups.clear-selection', defaultMessage: 'Clear' },
	emptyHeading: { id: 'craftpanel.backups.empty-heading', defaultMessage: 'No backups yet' },
	emptyDescription: {
		id: 'craftpanel.backups.empty-description',
		defaultMessage: 'A backup holds the whole server directory and can be restored at any time.',
	},
	creating: { id: 'craftpanel.backups.creating', defaultMessage: 'Creating backup...' },
	restoring: { id: 'craftpanel.backups.restoring', defaultMessage: 'Restoring backup...' },
	failed: { id: 'craftpanel.backups.failed', defaultMessage: 'The last attempt failed' },
	loadFailed: {
		id: 'craftpanel.backups.load-failed',
		defaultMessage: 'Failed to load the backups',
	},
	actionFailed: { id: 'craftpanel.backups.action-failed', defaultMessage: 'The action was refused' },
	deleted: { id: 'craftpanel.backups.deleted', defaultMessage: 'Backup deleted' },
	schedule: { id: 'craftpanel.backups.schedule', defaultMessage: 'Backup schedule' },
	scheduleHelp: {
		id: 'craftpanel.backups.schedule-help',
		defaultMessage: 'Automatic backups run in the background and clean up after themselves.',
	},
	every: { id: 'craftpanel.backups.every', defaultMessage: 'Every (hours)' },
	atHour: { id: 'craftpanel.backups.at-hour', defaultMessage: 'At (hour UTC)' },
	keepLast: { id: 'craftpanel.backups.keep-last', defaultMessage: 'Keep last' },
	nextRun: { id: 'craftpanel.backups.next-run', defaultMessage: 'Next run {time}' },
	restoreRunning: {
		id: 'craftpanel.backups.restore-running',
		defaultMessage: 'Wait until the running backup operation is done.',
	},
	restoreNotDone: {
		id: 'craftpanel.backups.restore-not-done',
		defaultMessage: 'Only a finished backup can be restored.',
	},
	targetTitle: { id: 'craftpanel.backups.target', defaultMessage: 'Where new backups go' },
	targetLocal: {
		id: 'craftpanel.backups.target.local',
		defaultMessage: 'Onto this machine.',
	},
	targetDrive: {
		id: 'craftpanel.backups.target.drive',
		defaultMessage: 'Into the Google Drive of whoever owns this server.',
	},
	targetNotConfigured: {
		id: 'craftpanel.backups.target.not-configured',
		defaultMessage: 'The operator has not set up Google Drive.',
	},
	targetNotConnected: {
		id: 'craftpanel.backups.target.not-connected',
		defaultMessage: 'The owner of this server has not connected a Google account.',
	},
	targetPolicy: {
		id: 'craftpanel.backups.target.policy',
		defaultMessage: 'The operator decided this for the whole panel.',
	},
	driveBrokenHeader: {
		id: 'craftpanel.backups.drive-broken',
		defaultMessage: 'Backups of this server cannot be made right now',
	},
	driveBrokenNotConnected: {
		id: 'craftpanel.backups.drive-broken.not-connected',
		defaultMessage:
			'This server backs up into its owner’s Google Drive, and no Google account is connected. The owner can connect one on his account page; until then every backup of this server fails.',
	},
	driveBrokenNotConnectedOwner: {
		id: 'craftpanel.backups.drive-broken.not-connected-owner',
		defaultMessage:
			'You need to connect a Google account before this server can be backed up: its backups go into your Google Drive, and until one is connected every backup of this server fails.',
	},
	driveBrokenNotConfigured: {
		id: 'craftpanel.backups.drive-broken.not-configured',
		defaultMessage:
			'This server backs up into a Google Drive, but the operator has taken the panel’s Google credentials away. Nothing can be uploaded or fetched back until they are entered again.',
	},
	driveConnect: {
		id: 'craftpanel.backups.drive-connect',
		defaultMessage: 'Connect Google Drive',
	},
	retryImpossible: {
		id: 'craftpanel.backups.retry-impossible',
		defaultMessage: 'A new attempt cannot get anywhere until there is a Google Drive to put it in.',
	},
	scheduleImpossible: {
		id: 'craftpanel.backups.schedule-impossible',
		defaultMessage:
			'An automatic backup cannot be switched on while this server has no Google Drive to back up into — it would fail every time.',
	},
	openInDrive: { id: 'craftpanel.backups.open-in-drive', defaultMessage: 'Open in Google Drive' },
	stateInDrive: { id: 'craftpanel.backups.state.in-drive', defaultMessage: 'In Google Drive' },
	stateMissing: { id: 'craftpanel.backups.state.missing', defaultMessage: 'Gone from the Drive' },
	stateTrashed: { id: 'craftpanel.backups.state.trashed', defaultMessage: 'In the Drive’s bin' },
	stateUnreachable: {
		id: 'craftpanel.backups.state.unreachable',
		defaultMessage: 'Drive not connected',
	},
	driveGoneHint: {
		id: 'craftpanel.backups.drive-gone-hint',
		defaultMessage: 'Put it back in your Drive to restore it from here.',
	},
	targetSwitched: { id: 'craftpanel.backups.target-switched', defaultMessage: 'Target changed' },
})

const TARGET_REASONS = {
	ok: messages.targetLocal,
	not_configured: messages.targetNotConfigured,
	not_connected: messages.targetNotConnected,
	policy: messages.targetPolicy,
} as const

const FILE_STATES = {
	present: messages.stateInDrive,
	missing: messages.stateMissing,
	trashed: messages.stateTrashed,
	unreachable: messages.stateUnreachable,
} as const

const FILE_COLORS = {
	present: 'green',
	missing: 'red',
	trashed: 'orange',
	unreachable: 'orange',
} as const

const queue = useServerBackupsQueue(ref(serverId), worldId)
const { query, backups, activeOperations, activeOperationByBackupId, invalidate } = queue

type Filter = 'all' | 'manual' | 'auto'
const filters: Filter[] = ['all', 'manual', 'auto']
const filter = ref<Filter>('all')
const selected = reactive(new Set<string>())
const schedule = ref<BackupSchedule | null>(null)
const target = ref<BackupTarget | null>(null)
const savingTarget = ref(false)

const createModal = ref<InstanceType<typeof BackupCreateModal>>()
const renameModal = ref<InstanceType<typeof BackupRenameModal>>()
const restoreModal = ref<InstanceType<typeof BackupRestoreModal>>()
const deleteModal = ref<InstanceType<typeof BackupDeleteModal>>()

const quota = computed(() => server.value?.backup_quota ?? 0)
const usedQuota = computed(() => server.value?.used_backup_quota ?? backups.value.length)
const keepLastMax = computed(() => Math.min(quota.value || MAX_BACKUP_QUOTA, MAX_BACKUP_QUOTA))
const errorText = computed(() =>
	query.error.value instanceof Error ? query.error.value.message : '',
)

const backupsTooltip = computed(() =>
	canManageBackups.value ? undefined : permissionDeniedMessage.value,
)

const cannotBackUp = computed(() => backupImpossible(target.value))

const isOwner = computed(() => user.value !== null && server.value?.owner_id === user.value.id)

const ownerCanConnect = computed(() => isOwner.value && needsOwnersDrive(target.value))

const driveBrokenBody = computed(() => {
	if (ownerCanConnect.value) return messages.driveBrokenNotConnectedOwner
	return target.value?.reason === 'not_configured'
		? messages.driveBrokenNotConfigured
		: messages.driveBrokenNotConnected
})

const createDisabled = computed(
	() =>
		!canManageBackups.value ||
		cannotBackUp.value ||
		usedQuota.value >= quota.value ||
		queue.hasActiveCreate.value,
)
const createTooltip = computed(() => {
	if (!canManageBackups.value) return permissionDeniedMessage.value
	if (cannotBackUp.value) return formatMessage(driveBrokenBody.value)
	if (usedQuota.value >= quota.value) return formatMessage(messages.quotaFull)
	if (queue.hasActiveCreate.value) return formatMessage(messages.busy)
	return undefined
})

const retryDisabled = computed(() => !canManageBackups.value || cannotBackUp.value)
const retryTooltip = computed(() => {
	if (!canManageBackups.value) return permissionDeniedMessage.value
	return cannotBackUp.value ? formatMessage(driveBrokenBody.value) : undefined
})

const scheduleLocked = computed(
	() =>
		!canManageBackups.value ||
		schedule.value === null ||
		(cannotBackUp.value && !schedule.value.enabled),
)

const visibleBackups = computed(() =>
	backups.value.filter((backup) => {
		if (filter.value === 'manual') return !backup.automated
		if (filter.value === 'auto') return backup.automated
		return true
	}),
)

const selectedBackups = computed(() => backups.value.filter((backup) => selected.has(backup.id)))

function filterLabel(value: Filter): string {
	return formatMessage(messages[value])
}

function toggle(id: string): void {
	if (selected.has(id)) selected.delete(id)
	else selected.add(id)
}

function runningOf(backup: QueueBackup): 'create' | 'restore' | null {
	return activeOperationByBackupId.value.get(backup.id)?.operation_type ?? null
}

const runningBackupOps = computed(() =>
	operations.value.filter(
		(entry) =>
			BACKUP_KINDS.has(entry.kind) && (entry.state === 'queued' || entry.state === 'ongoing'),
	),
)

function progressOf(backup: QueueBackup): number {
	return runningBackupOps.value.find((entry) => entry.target_id === backup.id)?.progress ?? 0
}

function failureOf(backup: QueueBackup): string | null {
	const last = backup.history[0]
	if (!last || (last.state !== 'failed' && last.state !== 'timed_out')) return null
	return last.error ?? formatMessage(messages.failed)
}

function failureBody(backup: QueueBackup): string {
	const why = failureOf(backup) ?? ''
	if (!cannotBackUp.value) return why
	return `${why} ${formatMessage(messages.retryImpossible)}`.trim()
}

function restoreDisabled(backup: QueueBackup): string | undefined {
	if (!canManageBackups.value) return permissionDeniedMessage.value
	if (backup.status !== 'done') return formatMessage(messages.restoreNotDone)
	if (activeOperations.value.length > 0) return formatMessage(messages.restoreRunning)
	if (notRestorable(driveFactsOf(backup))) return formatMessage(messages.driveGoneHint)
	return undefined
}

function report(cause: unknown): void {
	addNotification({
		type: 'error',
		title: formatMessage(messages.actionFailed),
		text: cause instanceof Error ? cause.message : undefined,
	})
}

const downloadHost = computed(() =>
	window.location.protocol === 'https:' ? window.location.host : undefined,
)

function download(backup: QueueBackup): void {
	window.location.assign(api.backups.downloadUrl(serverId, backup.id))
}

async function removeOne(backup: QueueBackup | undefined): Promise<void> {
	if (!backup) return
	try {
		await api.backups.remove(serverId, backup.id)
		selected.delete(backup.id)
		await invalidate()
		addNotification({ type: 'success', title: formatMessage(messages.deleted) })
	} catch (cause) {
		report(cause)
	}
}

async function removeMany(chosen: QueueBackup[]): Promise<void> {
	try {
		await api.backups.bulkDelete(serverId, { backup_ids: chosen.map((backup) => backup.id) })
		selected.clear()
		await invalidate()
	} catch (cause) {
		report(cause)
	}
}

async function retry(backupId: string): Promise<void> {
	try {
		await api.backups.retry(serverId, backupId)
		await invalidate()
	} catch (cause) {
		report(cause)
	}
}

async function loadSchedule(): Promise<void> {
	try {
		schedule.value = await api.backups.schedule(serverId)
	} catch (cause) {
		report(cause)
	}
}

async function loadTarget(): Promise<void> {
	try {
		target.value = await drive.target(serverId)
	} catch {
		target.value = null
	}
}

async function switchTarget(wanted: boolean | undefined): Promise<void> {
	if (savingTarget.value) return
	savingTarget.value = true
	try {
		target.value = await drive.setTarget(serverId, wanted ? 'drive' : 'local')
		addNotification({ type: 'success', title: formatMessage(messages.targetSwitched) })
	} catch (cause) {
		report(cause)
		await loadTarget()
	} finally {
		savingTarget.value = false
	}
}

function factsOf(backup: QueueBackup) {
	return driveFactsOf(backup)
}

function openInDrive(backup: QueueBackup): void {
	const link = driveFactsOf(backup).webLink
	if (link !== null) window.open(link, '_blank', 'noopener,noreferrer')
}

function within(
	value: string | number | undefined,
	min: number,
	max: number,
	patch: (parsed: number) => Partial<UpdateBackupScheduleRequest>,
): void {
	if (value === undefined || String(value).trim() === '') return
	const parsed = Number(value)
	if (!Number.isInteger(parsed) || parsed < min || parsed > max) return
	void saveSchedule(patch(parsed))
}

async function saveSchedule(patch: Partial<UpdateBackupScheduleRequest>): Promise<void> {
	const current = schedule.value
	if (current === null || !canManageBackups.value) return
	const next: UpdateBackupScheduleRequest = {
		enabled: current.enabled,
		interval_hours: current.interval_hours,
		hour_utc: current.hour_utc,
		keep_last: current.keep_last,
		...patch,
	}
	try {
		schedule.value = await api.backups.setSchedule(serverId, next)
	} catch (cause) {
		report(cause)
		await loadSchedule()
	}
}

watch(
	() => runningBackupOps.value.map((entry) => entry.id),
	(now, before) => {
		if (before.some((id) => !now.includes(id))) void invalidate()
	},
)

void loadSchedule()
void loadTarget()
</script>
