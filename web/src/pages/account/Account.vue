<template>
	<div class="mx-auto flex w-full max-w-[64rem] flex-col gap-4">
		<div class="flex flex-col gap-1">
			<h1 class="m-0 text-2xl font-extrabold text-contrast">
				{{ formatMessage(messages.title) }}
			</h1>
			<p class="m-0 text-secondary">{{ formatMessage(messages.subtitle) }}</p>
		</div>

		<LoadingIndicator v-if="me === null" />
		<template v-else>
			<Card class="!mb-0 flex flex-col gap-4">
				<div class="flex flex-wrap items-center gap-3">
					<CircleUserIcon aria-hidden="true" class="size-8 text-secondary" />
					<div class="flex min-w-0 flex-col">
						<span class="text-lg font-extrabold text-contrast">{{ me.username }}</span>
						<span class="text-sm text-secondary">
							{{ formatMessage(messages.systemAccount, { name: me.system_user.name }) }}
						</span>
					</div>
					<Badge
						:type="formatMessage(me.panel_role === 'admin' ? messages.roleAdmin : messages.roleUser)"
						:color="me.panel_role === 'admin' ? 'purple' : 'blue'"
						class="ml-auto"
					/>
				</div>
				<Admonition
					v-if="me.system_user.state !== 'ready'"
					type="warning"
					:body="formatMessage(messages.systemNotReady)"
				/>
			</Card>

			<Admonition
				v-if="me.usage.over_limit"
				type="critical"
				:header="formatMessage(messages.overLimitHeader)"
				:body="overLimitBody"
			/>

			<Card class="!mb-0 flex flex-col gap-6">
				<SettingsLabel
					:title="formatMessage(messages.limitsTitle)"
					:description="
						formatMessage(gauges.unlimited ? messages.noLimitsHint : messages.limitsHint)
					"
				/>

				<div class="flex flex-col gap-2">
					<div class="flex flex-wrap items-baseline gap-2">
						<span class="flex items-center gap-1.5 text-sm font-semibold text-contrast">
							<MemoryStickIcon aria-hidden="true" class="size-4" />
							{{ formatMessage(messages.memoryTitle) }}
						</span>
						<span
							class="ml-auto text-sm"
							:class="gauges.memory.over ? 'text-red' : 'text-secondary'"
						>
							{{ mibLabel(gauges.memory) }}
						</span>
					</div>
					<ProgressBar
						v-if="gauges.memory.percent !== null"
						:progress="gauges.memory.percent"
						:max="100"
						:color="gauges.memory.over ? 'red' : 'brand'"
						full-width
					/>
					<span class="text-xs text-secondary">
						{{
							formatMessage(messages.memoryDetail, {
								used: formatBytes(me.usage.memory.used_bytes),
							})
						}}
					</span>
				</div>

				<div class="flex flex-col gap-2">
					<div class="flex flex-wrap items-baseline gap-2">
						<span class="flex items-center gap-1.5 text-sm font-semibold text-contrast">
							<DatabaseIcon aria-hidden="true" class="size-4" />
							{{ formatMessage(messages.diskTitle) }}
						</span>
						<span class="ml-auto text-sm" :class="gauges.disk.over ? 'text-red' : 'text-secondary'">
							{{ diskLabel }}
						</span>
					</div>
					<ProgressBar
						v-if="gauges.disk.percent !== null"
						:progress="gauges.disk.percent"
						:max="100"
						:color="gauges.disk.over ? 'red' : 'brand'"
						full-width
					/>
					<span class="text-xs text-secondary">
						{{
							formatMessage(messages.diskDetail, {
								servers: formatBytes(me.usage.disk.servers_bytes),
								backups: formatBytes(me.usage.disk.backups_bytes),
							})
						}}
					</span>
					<span v-if="gauges.diskAtLeast" class="text-xs text-secondary">
						{{ formatMessage(messages.diskUnread) }}
					</span>
				</div>

				<dl class="m-0 grid grid-cols-[auto_1fr] gap-x-4 gap-y-2 text-sm">
					<dt class="flex items-center gap-1.5 text-secondary">
						<CpuIcon aria-hidden="true" class="size-4" />
						{{ formatMessage(messages.cpuTitle) }}
					</dt>
					<dd class="m-0 text-contrast">{{ cpuLabel }}</dd>

					<dt class="text-secondary">{{ formatMessage(messages.pidsTitle) }}</dt>
					<dd class="m-0 text-contrast">{{ pidsLabel }}</dd>
				</dl>
			</Card>

			<Card class="!mb-0 flex flex-col gap-4">
				<SettingsLabel
					:title="formatMessage(messages.serversTitle)"
					:description="
						formatMessage(messages.serversDescription, {
							total: me.usage.servers.total,
							running: me.usage.servers.running,
						})
					"
				/>
				<LoadingIndicator v-if="serversLoading" />
				<span v-else-if="serversFailure" class="text-sm text-red">{{ serversFailure }}</span>
				<p v-else-if="mine.length === 0" class="m-0 text-sm text-secondary">
					{{ formatMessage(messages.noServers) }}
				</p>
				<ul v-else class="m-0 flex list-none flex-col gap-2 p-0">
					<li
						v-for="server of mine"
						:key="server.id"
						class="flex flex-wrap items-center gap-2 rounded-lg bg-surface-2 px-3 py-1"
					>
						<RouterLink
							:to="{ name: 'server-overview', params: { id: server.id } }"
							class="min-w-0 truncate py-2.5 font-medium text-contrast hover:underline"
						>
							{{ server.name }}
						</RouterLink>
						<span class="ml-auto text-sm text-secondary">
							{{ formatMessage(messages.serverMemory, { mib: formatNumber(server.memory_mib) }) }}
						</span>
					</li>
				</ul>
			</Card>

			<component
				:is="section.component"
				v-for="section of accountSections"
				:key="section.id"
			/>
		</template>
	</div>
</template>

<script setup lang="ts">
import { CircleUserIcon, CpuIcon, DatabaseIcon, MemoryStickIcon } from '@modrinth/assets'
import {
	Admonition,
	Badge,
	Card,
	defineMessages,
	LoadingIndicator,
	ProgressBar,
	SettingsLabel,
	useFormatBytes,
	useFormatNumber,
	useVIntl,
} from '@modrinth/ui'
import { computed, onMounted, ref } from 'vue'
import { RouterLink } from 'vue-router'

import { api, isApiRequestError, type Server } from '@/api'
import { useSession } from '@/composables/session'

import { type Gauge, gaugesFor } from './limits'
import { accountSections } from './sections'

const { formatMessage } = useVIntl()
const formatBytes = useFormatBytes()
const formatNumber = useFormatNumber()
const { user: me, refresh } = useSession()

const MIB = 1024 * 1024

const messages = defineMessages({
	title: { id: 'account.title', defaultMessage: 'Your account' },
	subtitle: {
		id: 'account.subtitle',
		defaultMessage: 'What this account is allowed, and how much of it is in use.',
	},
	systemAccount: {
		id: 'account.system-account',
		defaultMessage: 'Runs as the system user {name}',
	},
	systemNotReady: {
		id: 'account.system-not-ready',
		defaultMessage:
			'The system user behind this account is not ready yet, so no server can start. An administrator can retry it.',
	},
	roleAdmin: { id: 'account.role.admin', defaultMessage: 'Administrator' },
	roleUser: { id: 'account.role.user', defaultMessage: 'User' },
	overLimitHeader: { id: 'account.over-limit.header', defaultMessage: 'Over the limit' },
	overLimitMemory: {
		id: 'account.over-limit.memory',
		defaultMessage:
			'More memory is handed out to servers than this account has. Nothing was stopped, but no new server can be created or started until a server is deleted or made smaller.',
	},
	overLimitDisk: {
		id: 'account.over-limit.disk',
		defaultMessage:
			'This account holds more on disk than it is allowed. Nothing was deleted and nothing was stopped, but uploads, installs and backups are refused until some room is freed.',
	},
	overLimitBoth: {
		id: 'account.over-limit.both',
		defaultMessage:
			'Both the memory budget and the disk limit are exceeded. Nothing was stopped or deleted, but nothing new can be taken up either.',
	},
	limitsTitle: { id: 'account.limits.title', defaultMessage: 'Limits' },
	limitsHint: {
		id: 'account.limits.hint',
		defaultMessage:
			'Memory is a budget you divide between your servers; disk space counts everything your servers and their backups hold together. Only an administrator can change these.',
	},
	noLimitsHint: {
		id: 'account.limits.none',
		defaultMessage:
			'This account has no limits at all: no memory ceiling, no processor cap, no process count and no disk quota. The figures below are what is measured, not what is allowed.',
	},
	memoryTitle: { id: 'account.memory.title', defaultMessage: 'Memory' },
	memoryDetail: { id: 'account.memory.detail', defaultMessage: 'In use right now: {used}' },
	diskTitle: { id: 'account.disk.title', defaultMessage: 'Disk space' },
	diskDetail: {
		id: 'account.disk.detail',
		defaultMessage: '{servers} in servers, {backups} in backups',
	},
	cpuTitle: { id: 'account.cpu.title', defaultMessage: 'Processor' },
	pidsTitle: { id: 'account.pids.title', defaultMessage: 'Processes' },
	ofMib: { id: 'account.of-mib', defaultMessage: '{value} of {total} MiB' },
	mibNoLimit: { id: 'account.mib-no-limit', defaultMessage: '{value} MiB, no limit' },
	ofBytes: { id: 'account.of-bytes', defaultMessage: '{used} of {total}' },
	bytesNoLimit: { id: 'account.bytes-no-limit', defaultMessage: '{used}, no limit' },
	atLeastOfBytes: { id: 'account.at-least-of-bytes', defaultMessage: 'at least {used} of {total}' },
	atLeastNoLimit: { id: 'account.at-least-no-limit', defaultMessage: 'at least {used}, no limit' },
	diskUnread: {
		id: 'account.disk.unread',
		defaultMessage:
			'Some folders were closed to the panel while it counted, so this is a lower bound. What is in them takes up disk all the same.',
	},
	coresCap: {
		id: 'account.cores.cap',
		defaultMessage: '{used, number} of {limit, plural, one {# core} other {# cores}}, hard cap',
	},
	coresShare: {
		id: 'account.cores.share',
		defaultMessage:
			'{used, number} of {limit, plural, one {# core} other {# cores}} while the machine is busy, more while it is idle',
	},
	coresNoLimit: {
		id: 'account.cores.none',
		defaultMessage: '{used, plural, one {# core} other {# cores}}, no limit',
	},
	pidsOf: { id: 'account.pids.of', defaultMessage: '{used} of {limit}' },
	pidsNoLimit: { id: 'account.pids.none', defaultMessage: '{used}, no limit' },
	serversTitle: { id: 'account.servers.title', defaultMessage: 'Where the budget went' },
	serversDescription: {
		id: 'account.servers.description',
		defaultMessage:
			'{total, plural, one {# server} other {# servers}}, {running, plural, =0 {none running} other {# running}}.',
	},
	serverMemory: { id: 'account.servers.memory', defaultMessage: '{mib} MiB' },
	noServers: { id: 'account.servers.none', defaultMessage: 'No servers of your own yet.' },
	unknownError: { id: 'account.error.unknown', defaultMessage: 'Something went wrong. Try again.' },
})

const servers = ref<Server[]>([])
const serversLoading = ref(true)
const serversFailure = ref<string | null>(null)

const gauges = computed(() => (me.value === null ? null : gaugesFor(me.value))!)

const mine = computed(() => servers.value.filter((server) => server.owner_id === me.value?.id))

const overLimitBody = computed(() => {
	const over = me.value?.usage.over_limit_dimensions ?? []
	const memory = over.includes('memory')
	const disk = over.includes('disk')
	if (memory && disk) return formatMessage(messages.overLimitBoth)
	return formatMessage(disk ? messages.overLimitDisk : messages.overLimitMemory)
})

function reason(error: unknown): string {
	return isApiRequestError(error) ? error.message : formatMessage(messages.unknownError)
}

function mibLabel(gauge: Gauge): string {
	if (gauge.limit === null) {
		return formatMessage(messages.mibNoLimit, { value: formatNumber(gauge.used) })
	}
	return formatMessage(messages.ofMib, {
		value: formatNumber(gauge.used),
		total: formatNumber(gauge.limit),
	})
}

const diskLabel = computed(() => {
	const disk = gauges.value.disk
	const used = formatBytes(me.value?.usage.disk.used_bytes ?? 0)
	const floor = gauges.value.diskAtLeast
	if (disk.limit === null) {
		return formatMessage(floor ? messages.atLeastNoLimit : messages.bytesNoLimit, { used })
	}
	const total = formatBytes(disk.limit * MIB, 0)
	return formatMessage(floor ? messages.atLeastOfBytes : messages.ofBytes, { used, total })
})

const cpuLabel = computed(() => {
	const used = Math.round(gauges.value.cpu.used * 100) / 100
	const limit = gauges.value.cpu.limit
	if (limit === null) return formatMessage(messages.coresNoLimit, { used })
	const capped = me.value?.limits?.cpu_mode === 'cap'
	return formatMessage(capped ? messages.coresCap : messages.coresShare, { used, limit })
})

const pidsLabel = computed(() => {
	const used = formatNumber(gauges.value.pids.used)
	const limit = gauges.value.pids.limit
	if (limit === null) return formatMessage(messages.pidsNoLimit, { used })
	return formatMessage(messages.pidsOf, { used, limit: formatNumber(limit) })
})

onMounted(() => {
	void refresh()
	void api.servers
		.list()
		.then((listed) => {
			servers.value = listed.servers
		})
		.catch((error) => {
			serversFailure.value = reason(error)
		})
		.finally(() => {
			serversLoading.value = false
		})
})
</script>
