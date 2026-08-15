<template>
	<section v-if="available" class="flex flex-col gap-2.5">
		<span class="text-lg font-semibold text-contrast">
			{{ formatMessage(messages.title) }}
		</span>

		<LoadingIndicator v-if="tunnel === null && loading" />

		<Admonition v-else-if="error" type="critical" :body="error">
			<template #actions>
				<Button @click="void refresh()">
					<UpdatedIcon aria-hidden="true" />
					{{ formatMessage(commonMessages.retryButton) }}
				</Button>
			</template>
		</Admonition>

		<template v-else-if="state === 'none'">
			<span class="text-secondary">{{ formatMessage(messages.noneBody) }}</span>

			<Admonition
				v-if="unconfigured"
				type="info"
				:body="
					formatMessage(isOwner ? messages.setupBodyOwner : messages.setupBody, {
						limit: FREE_PORTS,
					})
				"
			>
				<template v-if="isOwner" #actions>
					<Button @click="router.push({ name: 'account' })">
						<SettingsIcon aria-hidden="true" />
						{{ formatMessage(messages.setupButton) }}
					</Button>
				</template>
			</Admonition>

			<template v-else>
				<Admonition v-if="failure" type="critical" :body="failure" />
				<div v-if="canManage">
					<Button type="colored" color="brand" :disabled="busy" @click="request">
						<GlobeIcon aria-hidden="true" />
						{{ formatMessage(messages.request) }}
					</Button>
				</div>
				<span v-else class="text-sm text-secondary">
					{{ formatMessage(messages.ownerOnly) }}
				</span>
			</template>
		</template>

		<template v-else-if="state === 'pending'">
			<div class="flex items-center gap-3 text-secondary">
				<ProgressSpinner :progress="pendingProgress" class="size-5 text-brand" />
				<span>{{ formatMessage(messages.pending) }}</span>
			</div>
		</template>

		<template v-else>
			<CopyCode v-if="address" :text="address.address" />

			<details v-if="others.length" class="text-sm text-secondary">
				<summary class="w-fit cursor-pointer py-1">
					{{ formatMessage(messages.othersSummary, { count: others.length }) }}
				</summary>
				<div class="flex flex-col gap-1 pt-1">
					<span>{{ formatMessage(messages.othersHint) }}</span>
					<code v-for="entry in others" :key="entry.address" class="text-xs">
						{{ entry.address }}
					</code>
				</div>
			</details>

			<span v-if="showUnknownHostHint" class="text-sm text-secondary">
				{{ formatMessage(messages.unknownHostHint) }}
				<a
					class="text-link"
					href="https://playit.gg/support/minecraft-java-unknown-host/"
					target="_blank"
					rel="noopener noreferrer"
				>
					playit.gg
					<ExternalIcon aria-hidden="true" class="inline size-3" />
				</a>
			</span>
			<span v-else-if="state === 'online'" class="text-sm text-secondary">
				{{ formatMessage(messages.onlineHint) }}
			</span>

			<Admonition
				v-if="state !== 'online'"
				:type="state === 'offline' ? 'warning' : 'critical'"
				:header="formatMessage(STATE_HEADERS[state])"
				:body="tunnel?.detail ?? formatMessage(messages.stateUnknown)"
			>
				<template v-if="canManage" #actions>
					<Button :disabled="busy" @click="recreate">
						<UpdatedIcon aria-hidden="true" />
						{{ formatMessage(messages.recreate) }}
					</Button>
				</template>
			</Admonition>

			<Admonition v-if="failure" type="critical" :body="failure" />

			<div v-if="canManage" class="mt-1 flex flex-wrap items-center gap-3">
				<Button :disabled="busy" @click="removeModal?.show()">
					<UnlinkIcon aria-hidden="true" />
					{{ formatMessage(messages.giveBack) }}
				</Button>
				<span v-if="tunnel?.checked_at" class="text-sm text-secondary">
					{{ formatMessage(messages.checkedAt, { ago: relativeTime(tunnel.checked_at) }) }}
				</span>
			</div>
		</template>

		<ConfirmModal
			ref="removeModal"
			:title="formatMessage(messages.giveBack)"
			:description="formatMessage(messages.giveBackBody)"
			:proceed-label="formatMessage(messages.giveBack)"
			@proceed="remove"
		/>
	</section>
</template>

<script setup lang="ts">
import {
	ExternalIcon,
	GlobeIcon,
	SettingsIcon,
	UnlinkIcon,
	UpdatedIcon,
} from '@modrinth/assets'
import {
	Admonition,
	Button,
	commonMessages,
	ConfirmModal,
	CopyCode,
	defineMessages,
	LoadingIndicator,
	type MessageDescriptor,
	ProgressSpinner,
	useRelativeTime,
	useVIntl,
} from '@modrinth/ui'
import { useNow } from '@vueuse/core'
import { computed, ref } from 'vue'
import { useRouter } from 'vue-router'

import { isApiRequestError } from '@/api'
import { otherAddresses, playit, type PlayitTunnelState, shareAddress } from '@/api/playit'
import { useServerTunnel } from '@/composables/playit'
import { useServerPage } from '@/composables/server-page'
import { useSession } from '@/composables/session'

const { formatMessage } = useVIntl()
const relativeTime = useRelativeTime()
const router = useRouter()
const now = useNow({ interval: 1000 })
const { isAdmin, user } = useSession()
const { server, serverId } = useServerPage()
const {
	tunnel,
	loading,
	error,
	available,
	refresh,
	request: create,
	remove: drop,
} = useServerTunnel(serverId)

const FREE_PORTS = 4
const PENDING_SECONDS = 60

const messages = defineMessages({
	title: { id: 'craftpanel.playit.title', defaultMessage: 'Public address' },
	noneBody: {
		id: 'craftpanel.playit.none',
		defaultMessage:
			'This server has no address that works from outside. Friends can only reach it from this network.',
	},
	setupBody: {
		id: 'craftpanel.playit.setup-foreign',
		defaultMessage:
			'The panel can hand out an address through playit.gg that needs no port forwarding. The owner of this server has not connected a playit.gg account.',
	},
	setupBodyOwner: {
		id: 'craftpanel.playit.setup-owner',
		defaultMessage:
			'The panel can hand out an address through playit.gg that needs no port forwarding. Connect your own playit.gg account once — it is free — and up to {limit} of your servers can each get an address.',
	},
	setupButton: { id: 'craftpanel.playit.setup-button', defaultMessage: 'Set up playit.gg' },
	request: { id: 'craftpanel.playit.request', defaultMessage: 'Request public address' },
	ownerOnly: {
		id: 'craftpanel.playit.owner-only-account',
		defaultMessage:
			'Only the owner of this server can request one: it opens the server to the whole internet, and the address is handed out on his own playit.gg account.',
	},
	pending: {
		id: 'craftpanel.playit.pending',
		defaultMessage: 'The address is being set up at playit.gg…',
	},
	onlineHint: {
		id: 'craftpanel.playit.online-hint',
		defaultMessage: 'This is what your friends enter in their Minecraft client.',
	},
	othersSummary: {
		id: 'craftpanel.playit.others-summary',
		defaultMessage: '{count, plural, one {One more address} other {# more addresses}} from playit',
	},
	othersHint: {
		id: 'craftpanel.playit.others-hint',
		defaultMessage:
			'These point at the same relay, but Minecraft sends the address it was given and playit ' +
			'only forwards the tunnel’s own name, so they do not connect from the game.',
	},
	unknownHostHint: {
		id: 'craftpanel.playit.unknown-host',
		defaultMessage:
			'If a friend gets “Unknown host”, their internet provider does not resolve the name. It is fixed on their machine, by using another DNS server — playit explains how:',
	},
	offlineHeader: { id: 'craftpanel.playit.offline', defaultMessage: 'The address is not carrying' },
	missingHeader: {
		id: 'craftpanel.playit.missing',
		defaultMessage: 'The address was removed at playit.gg',
	},
	failedHeader: { id: 'craftpanel.playit.failed', defaultMessage: 'The address could not be set up' },
	stateUnknown: {
		id: 'craftpanel.playit.state-unknown',
		defaultMessage: 'playit.gg gave no reason.',
	},
	recreate: { id: 'craftpanel.playit.recreate', defaultMessage: 'Set up again' },
	giveBack: { id: 'craftpanel.playit.give-back', defaultMessage: 'Give the address back' },
	giveBackBody: {
		id: 'craftpanel.playit.give-back-body',
		defaultMessage:
			'The address stops working for everyone who has it, and the port goes back to the playit.gg account. The server itself is not touched.',
	},
	checkedAt: { id: 'craftpanel.playit.checked-at', defaultMessage: 'Last confirmed {ago}' },
	unknownError: {
		id: 'craftpanel.playit.unknown-error',
		defaultMessage: 'Something went wrong. Try again.',
	},
})

type BrokenState = Exclude<PlayitTunnelState, 'none' | 'pending' | 'online'>

const STATE_HEADERS: Record<BrokenState, MessageDescriptor> = {
	offline: messages.offlineHeader,
	missing: messages.missingHeader,
	failed: messages.failedHeader,
}

const busy = ref(false)
const failure = ref<string | null>(null)
const unconfigured = ref(false)
const removeModal = ref<InstanceType<typeof ConfirmModal> | null>(null)

const state = computed(() => tunnel.value?.state ?? 'none')
const address = computed(() => shareAddress(tunnel.value))
const others = computed(() => otherAddresses(tunnel.value))

const isOwner = computed(() => user.value?.id === server.value.owner_id)
const canManage = computed(() => isAdmin.value || isOwner.value)

const showUnknownHostHint = computed(
	() => state.value === 'online' && /[a-z]/i.test(address.value?.address ?? ''),
)

const pendingProgress = computed(() => {
	const started = Date.parse(tunnel.value?.created_at ?? '')
	if (!Number.isFinite(started)) return 0
	return Math.min(Math.max((now.value.getTime() - started) / (PENDING_SECONDS * 1000), 0), 1)
})

async function checkSetup(): Promise<void> {
	if (!isOwner.value) return
	const status = await playit.status().catch(() => null)
	unconfigured.value = status !== null && !status.configured
}

void checkSetup()

function reason(cause: unknown): string {
	return isApiRequestError(cause) ? cause.message : formatMessage(messages.unknownError)
}

async function request(): Promise<void> {
	if (busy.value) return
	busy.value = true
	failure.value = null
	try {
		await create()
	} catch (cause) {
		unconfigured.value = isApiRequestError(cause) && cause.code === 'playit_not_configured'
		failure.value = unconfigured.value ? null : reason(cause)
	} finally {
		busy.value = false
	}
}

async function remove(): Promise<void> {
	if (busy.value) return
	busy.value = true
	failure.value = null
	try {
		await drop()
	} catch (cause) {
		failure.value = reason(cause)
	} finally {
		busy.value = false
	}
}

async function recreate(): Promise<void> {
	if (busy.value) return
	busy.value = true
	failure.value = null
	try {
		await drop()
		await create()
	} catch (cause) {
		failure.value = reason(cause)
	} finally {
		busy.value = false
	}
}
</script>
