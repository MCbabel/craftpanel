<template>
	<Card class="!mb-0 flex flex-col gap-4">
		<div class="flex flex-wrap items-start justify-between gap-3">
			<SettingsLabel
				:title="formatMessage(messages.title)"
				:description="
					formatMessage(view.stage === 'unconnected' ? messages.intro : messages.connectedDescription)
				"
			/>
			<Badge
				v-if="status && view.stage !== 'unconnected'"
				:type="agentLabel"
				:color="AGENT_COLORS[status.agent.state]"
			/>
		</div>

		<LoadingIndicator v-if="status === null && loading" />

		<Admonition
			v-else-if="status === null"
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
				v-if="externalOff"
				type="warning"
				:header="formatMessage(messages.externalOffHeader)"
				:body="formatMessage(messages.externalOffBody)"
			/>

			<Admonition
				v-if="view.notSelfManaged"
				type="critical"
				:header="formatMessage(messages.notSelfManagedHeader)"
				:body="formatMessage(messages.notSelfManagedBody)"
			/>

			<Admonition v-if="actionFailure" type="critical" :body="actionFailure" />

			<template v-if="view.stage === 'unconnected'">
				<ul class="m-0 flex list-disc flex-col gap-1 pl-5 text-secondary">
					<li>{{ formatMessage(messages.pointOnce) }}</li>
					<li>{{ formatMessage(messages.pointPorts, { limit: FREE_PORTS }) }}</li>
					<li>{{ formatMessage(messages.pointPerServer) }}</li>
				</ul>

				<div class="flex flex-wrap items-center gap-3">
					<Button type="colored" color="brand" :disabled="busy" @click="beginClaim">
						<PlugIcon aria-hidden="true" />
						{{ formatMessage(messages.connectButton) }}
					</Button>
					<span class="text-sm text-secondary">{{ formatMessage(messages.connectFree) }}</span>
				</div>
			</template>

			<template v-else>
				<Admonition
					v-if="status.last_error"
					type="warning"
					:header="formatMessage(messages.lastErrorHeader)"
					:body="status.last_error"
				/>

				<div class="grid gap-4 sm:grid-cols-2 xl:grid-cols-3">
					<div class="flex flex-col gap-1.5 rounded-xl bg-surface-2 p-4">
						<span class="text-sm font-semibold text-secondary">
							{{ formatMessage(messages.factAgent) }}
						</span>
						<span class="text-lg font-extrabold text-contrast">{{ agentLabel }}</span>
						<span class="text-xs text-secondary">
							{{
								status.agent.detail ??
								formatMessage(view.stage === 'quiet' ? messages.quiet : messages.factAgentHint)
							}}
						</span>
					</div>

					<div class="flex flex-col gap-1.5 rounded-xl bg-surface-2 p-4">
						<span class="text-sm font-semibold text-secondary">
							{{ formatMessage(messages.factBinary) }}
						</span>
						<span class="text-lg font-extrabold text-contrast">{{ binaryLabel }}</span>
						<span class="text-xs text-secondary">
							{{
								status.binary.detail ??
								formatMessage(messages.factBinaryHint, {
									arch: status.binary.arch,
									version: status.binary.version ?? '—',
								})
							}}
						</span>
					</div>

					<div class="flex flex-col gap-1.5 rounded-xl bg-surface-2 p-4">
						<span class="text-sm font-semibold text-secondary">
							{{ formatMessage(messages.factPorts) }}
						</span>
						<span class="text-lg font-extrabold text-contrast">
							{{
								formatMessage(messages.factPortsValue, {
									used: formatNumber(view.ports.used),
									limit: formatNumber(view.ports.limit),
								})
							}}
						</span>
						<ProgressBar
							:progress="Math.min(view.ports.used, view.ports.limit)"
							:max="Math.max(view.ports.limit, 1)"
							:color="view.ports.full ? 'red' : 'brand'"
							full-width
						/>
						<span class="text-xs text-secondary">
							{{
								view.ports.full
									? formatMessage(messages.factPortsFull)
									: formatMessage(messages.factPortsLeft, {
											count: formatNumber(view.ports.free),
										})
							}}
						</span>
						<span v-if="view.ports.forOthers > 0" class="text-xs text-secondary">
							{{ formatMessage(messages.factPortsForOthers, { count: view.ports.forOthers }) }}
						</span>
					</div>
				</div>

				<div class="flex flex-col gap-2">
					<div class="flex flex-wrap items-center gap-2">
						<span class="text-sm font-semibold text-secondary">
							{{ formatMessage(messages.factAccount) }}
						</span>
						<Badge
							v-if="status.account_status"
							:type="accountLabel"
							:color="ACCOUNT_COLORS[status.account_status]"
						/>
						<span v-else class="text-sm text-secondary">
							{{ formatMessage(messages.factAccountUnknown) }}
						</span>
					</div>
					<span v-if="view.guest" class="text-sm text-secondary">
						{{ formatMessage(messages.guestHint) }}
						<a
							class="text-link"
							href="https://playit.gg/login"
							target="_blank"
							rel="noopener noreferrer"
						>
							playit.gg
							<ExternalIcon aria-hidden="true" class="inline size-3" />
						</a>
					</span>
				</div>

				<div class="flex flex-wrap items-center gap-x-6 gap-y-1 text-sm text-secondary">
					<span v-if="status.agent_id">
						{{ formatMessage(messages.factAgentId) }}
						<span class="font-mono text-contrast">{{ status.agent_id }}</span>
					</span>
					<span>
						{{
							status.checked_at
								? formatMessage(messages.checkedAt, { ago: relativeTime(status.checked_at) })
								: formatMessage(messages.checkedNever)
						}}
					</span>
				</div>

				<div class="flex flex-wrap gap-2">
					<Button v-if="view.stage === 'live'" :disabled="busy" @click="restartAgent">
						<UpdatedIcon aria-hidden="true" />
						{{ formatMessage(messages.restartAgent) }}
					</Button>
					<Button type="colored" color="red" :disabled="busy" @click="disconnectModal?.show()">
						<UnlinkIcon aria-hidden="true" />
						{{ formatMessage(messages.disconnect) }}
					</Button>
				</div>
			</template>
		</template>

		<NewModal
			ref="claimModal"
			:header="formatMessage(messages.claimTitle)"
			width="34rem"
			:on-hide="endClaim"
		>
			<div v-if="claim" class="flex flex-col gap-4">
				<p class="m-0 text-secondary">{{ formatMessage(messages.claimIntro) }}</p>

				<div class="flex flex-col gap-2">
					<a
						class="break-all text-lg font-semibold text-link"
						:href="claim.url"
						target="_blank"
						rel="noopener noreferrer"
					>
						{{ claim.url }}
						<ExternalIcon aria-hidden="true" class="inline size-4" />
					</a>
					<CopyCode :text="claim.url" />
				</div>

				<div v-if="phase === 'waiting'" class="flex items-center gap-3 text-secondary">
					<ProgressSpinner :progress="countdown.progress" class="size-6 text-brand" />
					<span>
						{{ formatMessage(messages.claimWaiting, { remaining: countdown.remaining }) }}
					</span>
				</div>

				<Admonition
					v-else-if="phase === 'accepted'"
					type="success"
					:body="formatMessage(messages.claimAccepted)"
				/>
				<Admonition
					v-else-if="phase === 'rejected'"
					type="critical"
					:body="formatMessage(messages.claimRejected)"
				/>
				<Admonition
					v-else
					type="warning"
					:body="formatMessage(messages.claimExpired, { minutes: CLAIM_MINUTES })"
				/>

				<Admonition v-if="claimFailure" type="warning" :body="claimFailure" />

				<div class="mb-1 mt-2 flex justify-end gap-2.5">
					<Button v-if="phase === 'waiting'" :disabled="busy" @click="claimModal?.hide()">
						<XIcon aria-hidden="true" />
						{{ formatMessage(commonMessages.cancelButton) }}
					</Button>
					<template v-else-if="phase !== 'accepted'">
						<Button :disabled="busy" @click="claimModal?.hide()">
							{{ formatMessage(commonMessages.cancelButton) }}
						</Button>
						<Button type="colored" color="brand" :disabled="busy" @click="restartClaim">
							<UpdatedIcon aria-hidden="true" />
							{{ formatMessage(messages.claimRetry) }}
						</Button>
					</template>
				</div>
			</div>
		</NewModal>

		<NewModal ref="disconnectModal" :header="formatMessage(messages.disconnect)" width="34rem">
			<div class="flex flex-col gap-4">
				<p class="m-0">{{ formatMessage(messages.disconnectBody) }}</p>
				<p v-if="view.ports.used > 0" class="m-0 text-secondary">
					{{ formatMessage(messages.disconnectTunnels, { count: view.ports.used }) }}
				</p>
				<Admonition v-if="disconnectFailure" type="critical" :body="disconnectFailure" />

				<div class="mb-1 mt-2 flex flex-wrap justify-end gap-2.5">
					<Button :disabled="busy" @click="disconnectModal?.hide()">
						{{ formatMessage(commonMessages.cancelButton) }}
					</Button>
					<Button v-if="view.ports.used > 0" :disabled="busy" @click="disconnect('keep')">
						{{ formatMessage(messages.disconnectKeep) }}
					</Button>
					<Button
						type="colored"
						color="red"
						:disabled="busy"
						@click="disconnect(view.ports.used > 0 ? 'delete' : undefined)"
					>
						<UnlinkIcon aria-hidden="true" />
						{{
							view.ports.used > 0
								? formatMessage(messages.disconnectDelete)
								: formatMessage(messages.disconnect)
						}}
					</Button>
				</div>
			</div>
		</NewModal>
	</Card>
</template>

<script setup lang="ts">
import { ExternalIcon, PlugIcon, UnlinkIcon, UpdatedIcon, XIcon } from '@modrinth/assets'
import {
	Admonition,
	Badge,
	Button,
	Card,
	commonMessages,
	CopyCode,
	defineMessages,
	LoadingIndicator,
	NewModal,
	ProgressBar,
	ProgressSpinner,
	SettingsLabel,
	useFormatNumber,
	useRelativeTime,
	useVIntl,
} from '@modrinth/ui'
import { useNow } from '@vueuse/core'
import { computed, onUnmounted, ref } from 'vue'

import { isApiRequestError } from '@/api'
import {
	claimPhase,
	notFound,
	playit,
	type PlayitClaim,
	type PlayitStatus,
	type PlayitTunnelDisposal,
	statusPollMs,
} from '@/api/playit'
import {
	ACCOUNT_COLORS,
	ACCOUNT_LABELS,
	AGENT_COLORS,
	AGENT_LABELS,
	BINARY_LABELS,
} from '@/components/playit-words'

import { claimCountdown, playitView } from './playit'

const { formatMessage } = useVIntl()
const formatNumber = useFormatNumber()
const relativeTime = useRelativeTime()
const now = useNow({ interval: 1000 })

const FREE_PORTS = 4
const CLAIM_MINUTES = 15
const CLAIM_POLL_MS = 2_000

const messages = defineMessages({
	title: { id: 'account.playit.title', defaultMessage: 'Public addresses for your servers' },
	intro: {
		id: 'account.playit.intro',
		defaultMessage:
			'playit.gg hands out an address that works from outside, without anyone touching a router. Connect your own account and your servers can ask for one.',
	},
	connectedDescription: {
		id: 'admin.playit.connected.description',
		defaultMessage: 'The tunnel service runs as a child of the panel and needs no attention.',
	},
	loadFailed: {
		id: 'admin.playit.load-failed',
		defaultMessage: 'Could not load the playit status',
	},
	unknownError: {
		id: 'admin.playit.unknown-error',
		defaultMessage: 'Something went wrong. Try again.',
	},
	externalOffHeader: {
		id: 'admin.playit.external-off.header',
		defaultMessage: 'Outbound services are switched off',
	},
	externalOffBody: {
		id: 'account.playit.external-off.body',
		defaultMessage:
			'playit.gg is an outbound service. While an administrator keeps that switch off, no address can be handed out and the tunnel service stays down.',
	},
	notSelfManagedHeader: {
		id: 'admin.playit.not-self-managed.header',
		defaultMessage: 'This agent may not manage its own tunnels',
	},
	notSelfManagedBody: {
		id: 'account.playit.not-self-managed.body',
		defaultMessage:
			'playit.gg reports the agent as not self-managed, so the panel cannot create addresses for your servers. Disconnect and connect again; the panel always asks for a self-managed agent.',
	},
	pointOnce: {
		id: 'admin.playit.connect.point-once',
		defaultMessage: 'You confirm once in the browser. No password is typed into this panel.',
	},
	pointPorts: {
		id: 'admin.playit.connect.point-ports',
		defaultMessage: 'A free account carries {limit} public addresses at the same time.',
	},
	pointPerServer: {
		id: 'admin.playit.connect.point-per-server',
		defaultMessage: 'Nothing changes for servers that do not ask for one.',
	},
	connectButton: { id: 'admin.playit.connect.button', defaultMessage: 'Connect to playit.gg' },
	connectFree: {
		id: 'admin.playit.connect.free',
		defaultMessage: 'Free, and it can be undone here at any time.',
	},
	lastErrorHeader: { id: 'admin.playit.last-error', defaultMessage: 'Last reported problem' },
	factAgent: { id: 'admin.playit.fact.agent', defaultMessage: 'Tunnel service' },
	factAgentHint: {
		id: 'admin.playit.fact.agent-hint',
		defaultMessage: 'Restarts with the panel; connected players see a short interruption.',
	},
	quiet: {
		id: 'account.playit.quiet',
		defaultMessage:
			'No server of yours has a public address, so the tunnel service is not running.',
	},
	factBinary: { id: 'admin.playit.fact.binary', defaultMessage: 'Program file' },
	factBinaryHint: {
		id: 'admin.playit.fact.binary-hint',
		defaultMessage: '{version} for {arch}, checked against a checksum built into the panel.',
	},
	factPorts: { id: 'admin.playit.fact.ports', defaultMessage: 'Public addresses' },
	factPortsValue: { id: 'admin.playit.fact.ports-value', defaultMessage: '{used} of {limit}' },
	factPortsLeft: { id: 'admin.playit.fact.ports-left', defaultMessage: '{count} still free.' },
	factPortsFull: {
		id: 'admin.playit.fact.ports-full',
		defaultMessage: 'All used up. A server can only get an address once another gives one back.',
	},
	factPortsForOthers: {
		id: 'account.playit.ports-for-others',
		defaultMessage:
			'{count, plural, one {One of them belongs to a server somebody else owns} other {# of them belong to servers somebody else owns}} — from the account this panel used to share. You can give those back on the server’s own page.',
	},
	factAccount: { id: 'admin.playit.fact.account', defaultMessage: 'playit.gg account' },
	factAccountUnknown: {
		id: 'admin.playit.fact.account-unknown',
		defaultMessage: 'Not known yet — waiting for the first answer from playit.gg.',
	},
	factAgentId: { id: 'admin.playit.fact.agent-id', defaultMessage: 'Agent' },
	guestHint: {
		id: 'admin.playit.guest-hint',
		defaultMessage:
			'A guest account is not tied to an e-mail address and can be lost. Sign in at',
	},
	checkedAt: { id: 'admin.playit.checked-at', defaultMessage: 'Last confirmed {ago}' },
	checkedNever: {
		id: 'admin.playit.checked-never',
		defaultMessage: 'Not confirmed by playit.gg yet',
	},
	restartAgent: { id: 'admin.playit.restart', defaultMessage: 'Restart tunnel service' },
	disconnect: { id: 'admin.playit.disconnect', defaultMessage: 'Disconnect' },
	disconnectBody: {
		id: 'account.playit.disconnect.body',
		defaultMessage:
			'The tunnel service stops and your key is deleted from this machine. Your servers stay up and stay reachable on the local network.',
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
	claimTitle: { id: 'admin.playit.claim.title', defaultMessage: 'Connect to playit.gg' },
	claimIntro: {
		id: 'admin.playit.claim.intro',
		defaultMessage: 'Open this address and confirm there. The page notices on its own.',
	},
	claimWaiting: {
		id: 'admin.playit.claim.waiting',
		defaultMessage: 'Waiting for the confirmation in the browser… {remaining} left',
	},
	claimAccepted: {
		id: 'admin.playit.claim.accepted',
		defaultMessage: 'Confirmed. The tunnel service is starting.',
	},
	claimRejected: {
		id: 'admin.playit.claim.rejected',
		defaultMessage: 'The request was turned down at playit.gg.',
	},
	claimExpired: {
		id: 'admin.playit.claim.expired',
		defaultMessage: 'Nothing was confirmed within {minutes} minutes, so the panel stopped asking.',
	},
	claimRetry: { id: 'admin.playit.claim.retry', defaultMessage: 'New link' },
})

const status = ref<PlayitStatus | null>(null)
const claim = ref<PlayitClaim | null>(null)
const loading = ref(true)
const busy = ref(false)
const externalOff = ref(false)
const loadFailure = ref<string | null>(null)
const actionFailure = ref<string | null>(null)
const claimFailure = ref<string | null>(null)
const disconnectFailure = ref<string | null>(null)
const claimModal = ref<InstanceType<typeof NewModal> | null>(null)
const disconnectModal = ref<InstanceType<typeof NewModal> | null>(null)

let statusTimer: ReturnType<typeof setTimeout> | undefined
let claimTimer: ReturnType<typeof setTimeout> | undefined
let resumable = true

const view = computed(() => playitView(status.value))
const agentLabel = computed(() =>
	formatMessage(AGENT_LABELS[status.value?.agent.state ?? 'absent']),
)
const binaryLabel = computed(() =>
	formatMessage(BINARY_LABELS[status.value?.binary.state ?? 'absent']),
)
const accountLabel = computed(() => {
	const account = status.value?.account_status
	return account ? formatMessage(ACCOUNT_LABELS[account]) : ''
})

const phase = computed(() => (claim.value ? claimPhase(claim.value, now.value.getTime()) : null))
const countdown = computed(() =>
	claim.value
		? claimCountdown(claim.value, now.value.getTime())
		: { remaining: '0:00', progress: 0 },
)

function reason(error: unknown): string {
	return isApiRequestError(error) ? error.message : formatMessage(messages.unknownError)
}

function note(error: unknown): void {
	if (isApiRequestError(error) && error.code === 'external_services_disabled') {
		externalOff.value = true
		actionFailure.value = null
		return
	}
	actionFailure.value = reason(error)
}

async function load(): Promise<void> {
	loading.value = true
	loadFailure.value = null
	try {
		adopt(await playit.status())
	} catch (error) {
		loadFailure.value = reason(error)
	} finally {
		loading.value = false
	}
}

function adopt(next: PlayitStatus): void {
	status.value = next
	loadFailure.value = null
	clearTimeout(statusTimer)
	statusTimer = setTimeout(() => void refresh(), statusPollMs(next))

	const running = next.claim
	if (resumable && running !== null && claim.value === null && claimPhase(running) === 'waiting') {
		claim.value = running
		claimModal.value?.show()
		pollClaim()
	}
}

async function refresh(): Promise<void> {
	try {
		adopt(await playit.status())
	} catch (error) {
		loadFailure.value = reason(error)
		clearTimeout(statusTimer)
		statusTimer = setTimeout(() => void refresh(), 20_000)
	}
}

void load()

onUnmounted(() => {
	clearTimeout(statusTimer)
	clearTimeout(claimTimer)
})

async function beginClaim(): Promise<void> {
	if (busy.value) return
	busy.value = true
	actionFailure.value = null
	claimFailure.value = null
	try {
		claim.value = await playit.startClaim()
		externalOff.value = false
		resumable = true
		claimModal.value?.show()
		pollClaim()
	} catch (error) {
		if (isApiRequestError(error) && error.code === 'playit_already_claimed') await refresh()
		else note(error)
	} finally {
		busy.value = false
	}
}

function pollClaim(): void {
	clearTimeout(claimTimer)
	claimTimer = setTimeout(() => void readClaim(), CLAIM_POLL_MS)
}

async function readClaim(): Promise<void> {
	try {
		claim.value = await playit.claim()
		claimFailure.value = null
	} catch (error) {
		if (notFound(error)) {
			await refresh()
			claimModal.value?.hide()
			return
		}
		claimFailure.value = reason(error)
	}

	if (phase.value === 'waiting') {
		pollClaim()
		return
	}
	if (phase.value === 'accepted') {
		await refresh()
		claimModal.value?.hide()
	}
}

async function endClaim(): Promise<void> {
	clearTimeout(claimTimer)
	resumable = false
	const open = claim.value
	claim.value = null
	if (open === null) return

	if (claimPhase(open) === 'waiting') {
		await playit.cancelClaim().catch(() => undefined)
	}
	await refresh()
}

async function restartClaim(): Promise<void> {
	clearTimeout(claimTimer)
	claim.value = null
	await playit.cancelClaim().catch(() => undefined)
	await beginClaim()
}

async function restartAgent(): Promise<void> {
	if (busy.value) return
	busy.value = true
	actionFailure.value = null
	try {
		adopt(await playit.restartAgent())
	} catch (error) {
		note(error)
	} finally {
		busy.value = false
	}
}

async function disconnect(tunnels?: PlayitTunnelDisposal): Promise<void> {
	if (busy.value) return
	busy.value = true
	disconnectFailure.value = null
	try {
		await playit.disconnect(tunnels)
		disconnectModal.value?.hide()
		await refresh()
	} catch (error) {
		disconnectFailure.value = reason(error)
	} finally {
		busy.value = false
	}
}
</script>
