<template>
	<div class="flex flex-col gap-6">
		<div class="flex flex-col gap-1">
			<h1 class="m-0 text-2xl font-extrabold text-contrast">
				{{ formatMessage(messages.title) }}
			</h1>
			<p class="m-0 text-secondary">{{ formatMessage(messages.subtitle) }}</p>
		</div>

		<Admonition
			v-if="externalServices === false"
			type="warning"
			:header="formatMessage(messages.externalOffHeader)"
			:body="formatMessage(messages.externalOffBody)"
		>
			<template #actions>
				<Button @click="router.push({ name: 'admin-settings' })">
					<SettingsIcon aria-hidden="true" />
					{{ formatMessage(messages.openSettings) }}
				</Button>
			</template>
		</Admonition>

		<Admonition
			v-if="actionFailure"
			type="critical"
			:body="actionFailure"
			dismissible
			@dismiss="actionFailure = null"
		/>

		<LoadingIndicator v-if="accounts === null && loading" />

		<Admonition
			v-else-if="accounts === null"
			type="critical"
			:header="formatMessage(messages.loadFailed)"
			:body="loadFailure ?? formatMessage(messages.unknownError)"
		>
			<template #actions>
				<Button @click="load()">
					<UpdatedIcon aria-hidden="true" />
					{{ formatMessage(commonMessages.retryButton) }}
				</Button>
			</template>
		</Admonition>

		<template v-else>
			<Admonition
				v-if="loadFailure"
				type="warning"
				:header="formatMessage(messages.loadFailed)"
				:body="loadFailure"
			/>

			<Table
				:columns="columns"
				:data="rows"
				row-key="user_id"
				:table-min-width="wide ? '60rem' : undefined"
				:row-below-visible="!wide"
			>
				<template #empty-state>
					<EmptyState
						type="empty"
						:heading="formatMessage(messages.nobody)"
						:description="formatMessage(messages.nobodyHint)"
					/>
				</template>

				<template #cell-user="{ index }">
					<div class="flex min-w-0 flex-col">
						<span class="truncate font-medium text-contrast">{{ nameOf(at(index)) }}</span>
						<span class="truncate text-xs text-secondary">{{ at(index).user_id }}</span>
					</div>
				</template>

				<template #cell-account="{ index }">
					<div class="flex flex-wrap items-center gap-1.5">
						<Badge
							v-if="at(index).account_status"
							:type="accountLabel(at(index))"
							:color="ACCOUNT_COLORS[at(index).account_status!]"
						/>
						<span v-else class="text-sm text-secondary">
							{{ formatMessage(messages.accountUnknown) }}
						</span>
						<Badge v-if="at(index).has_premium" :type="formatMessage(messages.premium)" color="purple" />
						<Badge
							v-if="!at(index).configured"
							:type="formatMessage(messages.notConfigured)"
							color="gray"
						/>
					</div>
				</template>

				<template #cell-agent="{ index }">
					<div class="flex min-w-0 flex-col">
						<Badge :type="agentLabel(at(index))" :color="AGENT_COLORS[at(index).agent.state]" />
						<span v-if="quiet(at(index))" class="mt-1 text-xs text-secondary">
							{{ formatMessage(messages.quiet) }}
						</span>
						<span v-else-if="at(index).agent.detail" class="mt-1 truncate text-xs text-secondary">
							{{ at(index).agent.detail }}
						</span>
					</div>
				</template>

				<template #cell-ports="{ index }">
					<div class="flex min-w-0 flex-col gap-1">
						<span class="text-sm" :class="full(at(index)) ? 'text-red' : 'text-contrast'">
							{{
								formatMessage(messages.portsValue, {
									used: formatNumber(at(index).ports.used),
									limit: formatNumber(at(index).ports.limit),
								})
							}}
						</span>
						<ProgressBar
							:progress="Math.min(at(index).ports.used, at(index).ports.limit)"
							:max="Math.max(at(index).ports.limit, 1)"
							:color="full(at(index)) ? 'red' : 'brand'"
							full-width
						/>
						<span v-if="at(index).ports.for_others > 0" class="text-xs text-secondary">
							{{
								formatMessage(messages.portsForOthers, { count: at(index).ports.for_others })
							}}
						</span>
					</div>
				</template>

				<template #cell-checked="{ index }">
					<div class="flex min-w-0 flex-col">
						<span class="text-sm text-contrast">
							{{
								at(index).checked_at
									? relativeTime(at(index).checked_at!)
									: formatMessage(messages.checkedNever)
							}}
						</span>
						<span v-if="at(index).last_error" class="truncate text-xs text-red">
							{{ at(index).last_error }}
						</span>
					</div>
				</template>

				<template #cell-actions="{ index }">
					<div class="flex items-center justify-end">
						<Button :disabled="busy" @click="ask(at(index))">
							<UnlinkIcon aria-hidden="true" />
							<span class="sr-only md:not-sr-only">{{ formatMessage(messages.disconnect) }}</span>
						</Button>
					</div>
				</template>

				<template #row-below="{ index }">
					<dl class="m-0 grid grid-cols-[auto_1fr] items-center gap-x-4 gap-y-2 px-4 pb-4 text-sm">
						<dt class="text-secondary">{{ formatMessage(messages.columnAccount) }}</dt>
						<dd class="m-0 flex flex-wrap items-center gap-1.5">
							<Badge
								v-if="at(index).account_status"
								:type="accountLabel(at(index))"
								:color="ACCOUNT_COLORS[at(index).account_status!]"
							/>
							<span v-else class="text-secondary">
								{{ formatMessage(messages.accountUnknown) }}
							</span>
							<Badge
								v-if="!at(index).configured"
								:type="formatMessage(messages.notConfigured)"
								color="gray"
							/>
						</dd>

						<dt class="text-secondary">{{ formatMessage(messages.columnAgent) }}</dt>
						<dd class="m-0">
							<Badge :type="agentLabel(at(index))" :color="AGENT_COLORS[at(index).agent.state]" />
						</dd>

						<dt class="text-secondary">{{ formatMessage(messages.columnPorts) }}</dt>
						<dd class="m-0 flex min-w-0 flex-col">
							<span :class="full(at(index)) ? 'text-red' : 'text-contrast'">
								{{
									formatMessage(messages.portsValue, {
										used: formatNumber(at(index).ports.used),
										limit: formatNumber(at(index).ports.limit),
									})
								}}
							</span>
							<span v-if="at(index).ports.for_others > 0" class="text-xs text-secondary">
								{{
									formatMessage(messages.portsForOthers, { count: at(index).ports.for_others })
								}}
							</span>
						</dd>

						<dt class="text-secondary">{{ formatMessage(messages.columnChecked) }}</dt>
						<dd class="m-0 flex min-w-0 flex-col">
							<span class="text-contrast">
								{{
									at(index).checked_at
										? relativeTime(at(index).checked_at!)
										: formatMessage(messages.checkedNever)
								}}
							</span>
							<span v-if="at(index).last_error" class="text-xs text-red">
								{{ at(index).last_error }}
							</span>
						</dd>
					</dl>
				</template>
			</Table>

			<p class="m-0 text-sm text-secondary">{{ formatMessage(messages.ownAccountHint) }}</p>
		</template>

		<NewModal ref="disconnectModal" :header="formatMessage(messages.disconnect)" width="34rem">
			<div class="flex flex-col gap-4">
				<p class="m-0">
					{{ formatMessage(messages.disconnectBody, { user: nameOf(chosen) }) }}
				</p>
				<p v-if="chosenTunnels > 0" class="m-0 text-secondary">
					{{ formatMessage(messages.disconnectTunnels, { count: chosenTunnels }) }}
				</p>
				<Admonition v-if="disconnectFailure" type="critical" :body="disconnectFailure" />

				<div class="mb-1 mt-2 flex flex-wrap justify-end gap-2.5">
					<Button :disabled="busy" @click="disconnectModal?.hide()">
						{{ formatMessage(commonMessages.cancelButton) }}
					</Button>
					<Button v-if="chosenTunnels > 0" :disabled="busy" @click="disconnect('keep')">
						{{ formatMessage(messages.disconnectKeep) }}
					</Button>
					<Button
						type="colored"
						color="red"
						:disabled="busy"
						@click="disconnect(chosenTunnels > 0 ? 'delete' : undefined)"
					>
						<UnlinkIcon aria-hidden="true" />
						{{
							chosenTunnels > 0
								? formatMessage(messages.disconnectDelete)
								: formatMessage(messages.disconnect)
						}}
					</Button>
				</div>
			</div>
		</NewModal>
	</div>
</template>

<script setup lang="ts">
import { SettingsIcon, UnlinkIcon, UpdatedIcon } from '@modrinth/assets'
import {
	Admonition,
	Badge,
	Button,
	commonMessages,
	defineMessages,
	EmptyState,
	LoadingIndicator,
	NewModal,
	ProgressBar,
	type TableColumn,
	Table,
	useFormatNumber,
	useRelativeTime,
	useVIntl,
} from '@modrinth/ui'
import { computed, onUnmounted, ref } from 'vue'
import { useRouter } from 'vue-router'

import { api, isApiRequestError, type Ulid } from '@/api'
import {
	playit,
	type PlayitOverview,
	type PlayitTunnelDisposal,
	portsLeft,
} from '@/api/playit'
import { ACCOUNT_COLORS, ACCOUNT_LABELS, AGENT_COLORS, AGENT_LABELS } from '@/components/playit-words'
import { actionsColumnWidth, ICON_LABEL_BUTTON_REM } from '@/components/table-widths'
import { useWideScreen } from '@/composables/breakpoint'

const { formatMessage } = useVIntl()
const formatNumber = useFormatNumber()
const relativeTime = useRelativeTime()
const router = useRouter()
const wide = useWideScreen()

const POLL_MS = 20_000

const messages = defineMessages({
	title: { id: 'admin.playit.title-all', defaultMessage: 'Public addresses (all users)' },
	subtitle: {
		id: 'admin.playit.subtitle-all',
		defaultMessage:
			'Every user connects his own playit.gg account, and his ports are his. This is who has one — you can look, and you can cut somebody loose.',
	},
	loadFailed: { id: 'admin.playit.load-failed', defaultMessage: 'Could not load the playit status' },
	unknownError: {
		id: 'admin.playit.unknown-error',
		defaultMessage: 'Something went wrong. Try again.',
	},
	externalOffHeader: {
		id: 'admin.playit.external-off.header',
		defaultMessage: 'Outbound services are switched off',
	},
	externalOffBody: {
		id: 'admin.playit.external-off.body',
		defaultMessage:
			'playit.gg is an outbound service. While the switch under panel settings is off, the agent stays down and nothing can be connected.',
	},
	openSettings: { id: 'admin.playit.open-settings', defaultMessage: 'Panel settings' },
	nobody: { id: 'admin.playit.nobody', defaultMessage: 'Nobody has connected an account' },
	nobodyHint: {
		id: 'admin.playit.nobody-hint',
		defaultMessage:
			'A user who wants an address that works from outside connects his own playit.gg account on his account page. Nothing has to be set up here.',
	},
	ownAccountHint: {
		id: 'admin.playit.own-account-hint',
		defaultMessage:
			'Your own account is on your account page. Nobody can connect one for somebody else — the confirmation happens in that person’s browser at playit.gg.',
	},
	columnUser: { id: 'admin.playit.column.user', defaultMessage: 'User' },
	columnAccount: { id: 'admin.playit.column.account', defaultMessage: 'playit.gg account' },
	columnAgent: { id: 'admin.playit.column.agent', defaultMessage: 'Tunnel service' },
	columnPorts: { id: 'admin.playit.column.ports', defaultMessage: 'Public addresses' },
	columnChecked: { id: 'admin.playit.column.checked', defaultMessage: 'Last confirmed' },
	columnActions: { id: 'admin.playit.column.actions', defaultMessage: 'Actions' },
	accountUnknown: {
		id: 'admin.playit.fact.account-unknown',
		defaultMessage: 'Not known yet — waiting for the first answer from playit.gg.',
	},
	premium: { id: 'admin.playit.premium', defaultMessage: 'premium' },
	notConfigured: { id: 'admin.playit.not-configured', defaultMessage: 'no key' },
	quiet: {
		id: 'admin.playit.quiet',
		defaultMessage: 'No server of his has an address, so nothing is running.',
	},
	portsValue: { id: 'admin.playit.fact.ports-value', defaultMessage: '{used} of {limit}' },
	portsForOthers: {
		id: 'admin.playit.ports-for-others',
		defaultMessage:
			'{count, plural, one {# for a server somebody else owns} other {# for servers other people own}}',
	},
	checkedNever: {
		id: 'admin.playit.checked-never',
		defaultMessage: 'Not confirmed by playit.gg yet',
	},
	unnamed: { id: 'admin.playit.unnamed', defaultMessage: 'Account already deleted' },
	disconnect: { id: 'admin.playit.disconnect', defaultMessage: 'Disconnect' },
	disconnectBody: {
		id: 'admin.playit.disconnect.body-user',
		defaultMessage:
			'The tunnel service of {user} stops and the key is deleted from this machine. His servers stay up and stay reachable on the local network. He can connect an account again himself at any time.',
	},
	disconnectTunnels: {
		id: 'admin.playit.disconnect.tunnels',
		defaultMessage:
			'{count} address(es) are handed out. Delete them at playit.gg so their ports are free again, or keep them if playit.gg cannot be reached right now — they will then keep occupying ports on the account.',
	},
	disconnectDelete: {
		id: 'admin.playit.disconnect.delete',
		defaultMessage: 'Delete addresses and disconnect',
	},
	disconnectKeep: { id: 'admin.playit.disconnect.keep', defaultMessage: 'Keep addresses' },
})

type PlayitRow = { user_id: Ulid; line: PlayitOverview }
type PlayitColumn = 'user' | 'account' | 'agent' | 'ports' | 'checked' | 'actions'

const accounts = ref<PlayitOverview[] | null>(null)
const externalServices = ref<boolean | null>(null)
const chosen = ref<PlayitOverview | null>(null)
const loading = ref(true)
const busy = ref(false)
const loadFailure = ref<string | null>(null)
const actionFailure = ref<string | null>(null)
const disconnectFailure = ref<string | null>(null)
const disconnectModal = ref<InstanceType<typeof NewModal> | null>(null)

let timer: ReturnType<typeof setTimeout> | undefined

const rows = computed<PlayitRow[]>(() =>
	(accounts.value ?? []).map((line) => ({ user_id: line.user_id, line })),
)

const columns = computed<TableColumn<PlayitColumn>[]>(() =>
	wide.value
		? [
				{ key: 'user', label: formatMessage(messages.columnUser), width: '18rem' },
				{ key: 'account', label: formatMessage(messages.columnAccount), width: '12rem' },
				{ key: 'agent', label: formatMessage(messages.columnAgent), width: '12rem' },
				{ key: 'ports', label: formatMessage(messages.columnPorts), width: '12rem' },
				{ key: 'checked', label: formatMessage(messages.columnChecked), width: '12rem' },
				{
					key: 'actions',
					label: formatMessage(messages.columnActions),
					align: 'right',
					width: '9rem',
				},
			]
		: [
				{ key: 'user', label: formatMessage(messages.columnUser) },
				{ key: 'actions', align: 'right', width: actionsColumnWidth([ICON_LABEL_BUTTON_REM]) },
			],
)

function at(index: number): PlayitOverview {
	return rows.value[index]!.line
}

function nameOf(line: PlayitOverview | null): string {
	return line?.username ?? formatMessage(messages.unnamed)
}

function agentLabel(line: PlayitOverview): string {
	return formatMessage(AGENT_LABELS[line.agent.state])
}

function accountLabel(line: PlayitOverview): string {
	return line.account_status ? formatMessage(ACCOUNT_LABELS[line.account_status]) : ''
}

function full(line: PlayitOverview): boolean {
	return portsLeft(line) === 0
}

function quiet(line: PlayitOverview): boolean {
	return line.configured && line.ports.used === 0 && line.agent.state === 'absent'
}

const chosenTunnels = computed(() => chosen.value?.ports.used ?? 0)

function reason(error: unknown): string {
	return isApiRequestError(error) ? error.message : formatMessage(messages.unknownError)
}

async function load(): Promise<void> {
	loading.value = true
	loadFailure.value = null
	try {
		const [list, settings] = await Promise.all([
			playit.overview(),
			api.admin.settings().catch(() => null),
		])
		accounts.value = list
		externalServices.value = settings?.external_services_enabled ?? null
	} catch (error) {
		loadFailure.value = reason(error)
	} finally {
		loading.value = false
		clearTimeout(timer)
		timer = setTimeout(() => void refresh(), POLL_MS)
	}
}

async function refresh(): Promise<void> {
	try {
		accounts.value = await playit.overview()
		loadFailure.value = null
	} catch (error) {
		loadFailure.value = reason(error)
	} finally {
		clearTimeout(timer)
		timer = setTimeout(() => void refresh(), POLL_MS)
	}
}

void load()

onUnmounted(() => clearTimeout(timer))

function ask(line: PlayitOverview): void {
	chosen.value = line
	disconnectFailure.value = null
	disconnectModal.value?.show()
}

async function disconnect(tunnels?: PlayitTunnelDisposal): Promise<void> {
	const line = chosen.value
	if (busy.value || line === null) return
	busy.value = true
	disconnectFailure.value = null
	try {
		await playit.disconnectUser(line.user_id, tunnels)
		disconnectModal.value?.hide()
		await refresh()
	} catch (error) {
		disconnectFailure.value = reason(error)
	} finally {
		busy.value = false
	}
}
</script>
