<template>
	<div class="flex flex-col gap-6">
		<div class="flex flex-col gap-2">
			<SettingsLabel
				:title="formatMessage(messages.memoryTitle)"
				:description="formatMessage(messages.memoryDescription)"
			/>
			<Slider
				:model-value="limits.memory_mib"
				:min="MEMORY_MIN"
				:max="memoryCeiling"
				:step="MEMORY_STEP"
				unit="MiB"
				:disabled="disabled"
				@update:model-value="(value) => update({ memory_mib: value })"
			/>
			<p v-if="usage" class="m-0 text-sm text-secondary">
				<span :class="overAllocated ? 'text-red' : ''">
					{{
						formatMessage(messages.memoryAllocated, {
							allocated: formatNumber(usage.memory.allocated_mib),
							limit: formatNumber(limits.memory_mib),
						})
					}}
				</span>
				<span> · </span>
				<span>
					{{
						formatMessage(messages.memoryUsed, { used: formatBytes(usage.memory.used_bytes) })
					}}
				</span>
			</p>
			<Admonition
				v-if="overAllocated"
				type="warning"
				:body="formatMessage(messages.belowAllocated)"
			/>
		</div>

		<div class="flex flex-col gap-2">
			<SettingsLabel
				:title="formatMessage(messages.cpuTitle)"
				:description="
					formatMessage(limits.cpu_mode === 'cap' ? messages.cpuCapHint : messages.cpuShareHint, {
						cores: formatNumber(limits.cpu_cores),
					})
				"
			/>
			<div class="flex flex-wrap items-end gap-3">
				<div class="flex flex-col gap-1.5">
					<label class="text-sm font-semibold text-contrast" :for="`${scope}-cpu-cores`">
						{{ formatMessage(messages.coresLabel) }}
					</label>
					<StyledInput
						:id="`${scope}-cpu-cores`"
						:model-value="limits.cpu_cores"
						type="number"
						:min="CPU_MIN"
						:step="0.5"
						:disabled="disabled"
						wrapper-class="w-28"
						@update:model-value="setCores"
					/>
				</div>
				<div class="flex min-w-[12rem] flex-col gap-1.5">
					<span class="text-sm font-semibold text-contrast">
						{{ formatMessage(messages.cpuModeLabel) }}
					</span>
					<DropdownSelect
						:model-value="limits.cpu_mode"
						:options="CPU_MODES"
						:display-name="cpuModeName"
						:name="`${scope}-cpu-mode`"
						:disabled="disabled"
						class="!h-9 !w-full"
						@update:model-value="setMode"
					/>
				</div>
			</div>
			<p v-if="usage" class="m-0 text-sm text-secondary">
				{{
					usage.cpu.limit_cores === null
						? formatMessage(messages.cpuUsedOnly, {
								used: formatNumber(roundCores(usage.cpu.used_cores)),
							})
						: formatMessage(messages.cpuUsed, {
								used: formatNumber(roundCores(usage.cpu.used_cores)),
								limit: formatNumber(usage.cpu.limit_cores),
							})
				}}
			</p>
		</div>

		<div class="flex flex-col gap-2">
			<SettingsLabel
				:title="formatMessage(messages.pidsTitle)"
				:description="formatMessage(messages.pidsDescription)"
			/>
			<Slider
				:model-value="limits.pids_max"
				:min="PIDS_MIN"
				:max="PIDS_MAX"
				:step="PIDS_STEP"
				:disabled="disabled"
				@update:model-value="(value) => update({ pids_max: value })"
			/>
			<p v-if="usage" class="m-0 text-sm text-secondary">
				{{
					usage.pids.limit === null
						? formatMessage(messages.pidsUsedOnly, { used: formatNumber(usage.pids.used) })
						: formatMessage(messages.pidsUsed, {
								used: formatNumber(usage.pids.used),
								limit: formatNumber(usage.pids.limit),
							})
				}}
			</p>
		</div>

		<div class="flex flex-col gap-2">
			<SettingsLabel
				:title="formatMessage(messages.diskTitle)"
				:description="formatMessage(messages.diskDescription)"
			/>
			<Slider
				:model-value="limits.disk_mib"
				:min="DISK_MIN"
				:max="diskCeiling"
				:step="DISK_STEP"
				unit="MiB"
				:disabled="disabled"
				@update:model-value="(value) => update({ disk_mib: value })"
			/>
			<p v-if="usage" class="m-0 text-sm text-secondary">
				<span :class="overDisk ? 'text-red' : ''">
					{{
						formatMessage(
							usage.disk.complete === false ? messages.diskUsedAtLeast : messages.diskUsed,
							{
								used: formatBytes(usage.disk.used_bytes),
								limit: formatNumber(limits.disk_mib),
							},
						)
					}}
				</span>
				<span> · </span>
				<span>
					{{
						formatMessage(messages.diskSplit, {
							servers: formatBytes(usage.disk.servers_bytes),
							backups: formatBytes(usage.disk.backups_bytes),
						})
					}}
				</span>
			</p>
			<Admonition v-if="overDisk" type="warning" :body="formatMessage(messages.overDisk)" />
		</div>
	</div>
</template>

<script setup lang="ts">
import {
	Admonition,
	defineMessages,
	DropdownSelect,
	SettingsLabel,
	Slider,
	StyledInput,
	useFormatBytes,
	useFormatNumber,
	useVIntl,
} from '@modrinth/ui'
import { computed } from 'vue'

import type { CpuMode, UserLimits, UserUsage } from '@/api'

const { formatMessage } = useVIntl()
const formatBytes = useFormatBytes()
const formatNumber = useFormatNumber()

const MEMORY_MIN = 512
const MEMORY_STEP = 256
const CPU_MIN = 0.5
const PIDS_MIN = 64
const PIDS_MAX = 8192
const PIDS_STEP = 64
const DISK_MIN = 1024
const DISK_STEP = 1024
const CPU_MODES: CpuMode[] = ['cap', 'share']

const messages = defineMessages({
	memoryTitle: {
		id: 'admin.limits.memory.title',
		defaultMessage: 'Memory',
	},
	memoryDescription: {
		id: 'admin.limits.memory.description',
		defaultMessage: 'The ceiling for everything this account runs, in mebibytes.',
	},
	memoryAllocated: {
		id: 'admin.limits.memory.allocated',
		defaultMessage: 'Handed out to servers: {allocated} of {limit} MiB',
	},
	memoryUsed: {
		id: 'admin.limits.memory.used',
		defaultMessage: 'in use right now: {used}',
	},
	belowAllocated: {
		id: 'admin.limits.memory.below-allocated',
		defaultMessage:
			'This is below what the account has already handed out to its servers. Saving is allowed and nothing is killed, but the account counts as over limit until a server is deleted or shrunk, and it cannot start anything new.',
	},
	cpuTitle: {
		id: 'admin.limits.cpu.title',
		defaultMessage: 'Processor',
	},
	cpuCapHint: {
		id: 'admin.limits.cpu.cap-hint',
		defaultMessage:
			'Hard cap: the account never gets more than {cores} cores, however idle the machine is.',
	},
	cpuShareHint: {
		id: 'admin.limits.cpu.share-hint',
		defaultMessage:
			'Share: {cores} cores worth while the machine is contended, more while it is idle. No ceiling.',
	},
	coresLabel: {
		id: 'admin.limits.cpu.cores',
		defaultMessage: 'Cores',
	},
	cpuModeLabel: {
		id: 'admin.limits.cpu.mode',
		defaultMessage: 'Mode',
	},
	cpuModeCap: {
		id: 'admin.limits.cpu.mode.cap',
		defaultMessage: 'Hard cap',
	},
	cpuModeShare: {
		id: 'admin.limits.cpu.mode.share',
		defaultMessage: 'Share',
	},
	cpuUsed: {
		id: 'admin.limits.cpu.used',
		defaultMessage: 'in use right now: {used} of {limit} cores',
	},
	pidsTitle: {
		id: 'admin.limits.pids.title',
		defaultMessage: 'Processes',
	},
	pidsDescription: {
		id: 'admin.limits.pids.description',
		defaultMessage: 'How many processes and threads the account may have alive at once.',
	},
	pidsUsed: {
		id: 'admin.limits.pids.used',
		defaultMessage: 'in use right now: {used} of {limit}',
	},
	pidsUsedOnly: {
		id: 'admin.limits.pids.used-only',
		defaultMessage: 'in use right now: {used}',
	},
	cpuUsedOnly: {
		id: 'admin.limits.cpu.used-only',
		defaultMessage: 'in use right now: {used} cores',
	},
	diskTitle: {
		id: 'admin.limits.disk.title',
		defaultMessage: 'Disk space',
	},
	diskDescription: {
		id: 'admin.limits.disk.description',
		defaultMessage:
			'Everything this account keeps on disk: all its servers plus their backups. The kernel has no ceiling for disk space, so the panel refuses uploads, installs and backups once the limit is reached; nothing is deleted and no server is stopped.',
	},
	diskUsed: {
		id: 'admin.limits.disk.used',
		defaultMessage: 'In use: {used} of {limit} MiB',
	},
	diskUsedAtLeast: {
		id: 'admin.limits.disk.used-at-least',
		defaultMessage: 'In use: at least {used} of {limit} MiB — some folders were closed to the panel',
	},
	diskSplit: {
		id: 'admin.limits.disk.split',
		defaultMessage: '{servers} in servers, {backups} in backups',
	},
	overDisk: {
		id: 'admin.limits.disk.over',
		defaultMessage:
			'This is below what the account already holds. Saving is allowed and nothing is deleted, but the account can take up no more room until it frees some.',
	},
})

const props = withDefaults(
	defineProps<{
		scope: string
		memoryMax: number
		diskMax: number
		usage?: UserUsage | null
		disabled?: boolean
	}>(),
	{
		usage: null,
		disabled: false,
	},
)

const limits = defineModel<UserLimits>({ required: true })

const memoryCeiling = computed(() =>
	Math.max(props.memoryMax, limits.value.memory_mib, MEMORY_MIN + MEMORY_STEP),
)

const diskCeiling = computed(() =>
	Math.max(props.diskMax, limits.value.disk_mib, DISK_MIN + DISK_STEP),
)

const overAllocated = computed(
	() => props.usage !== null && limits.value.memory_mib < props.usage.memory.allocated_mib,
)

const MIB = 1024 * 1024

const overDisk = computed(
	() => props.usage !== null && limits.value.disk_mib * MIB < props.usage.disk.used_bytes,
)

function update(patch: Partial<UserLimits>): void {
	limits.value = { ...limits.value, ...patch }
}

function cpuModeName(mode: CpuMode): string {
	return formatMessage(mode === 'cap' ? messages.cpuModeCap : messages.cpuModeShare)
}

function setMode(value: unknown): void {
	if (value === 'cap' || value === 'share') update({ cpu_mode: value })
}

function setCores(value: string | number | undefined): void {
	const cores = typeof value === 'number' ? value : Number(value)
	if (Number.isFinite(cores) && cores > 0) update({ cpu_cores: cores })
}

function roundCores(value: number): number {
	return Math.round(value * 100) / 100
}
</script>
