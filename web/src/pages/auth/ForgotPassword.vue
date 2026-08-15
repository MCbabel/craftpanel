<template>
	<div class="mx-auto flex min-h-full w-full max-w-[27rem] flex-col justify-center px-6 py-12">
		<Card class="!mb-0 flex flex-col gap-6">
			<div class="flex flex-col gap-1">
				<h1 class="m-0 text-2xl font-extrabold text-contrast">
					{{ formatMessage(messages.title) }}
				</h1>
				<p class="m-0 text-secondary">{{ formatMessage(messages.subtitle) }}</p>
			</div>

			<div v-if="options === null && !failed" class="flex items-center gap-2 text-secondary">
				<SpinnerIcon class="animate-spin" />
				{{ formatMessage(messages.loading) }}
			</div>

			<template v-else-if="!available">
				<Admonition type="info" :body="formatMessage(messages.notSetUp)" />
				<ButtonLink :to="{ name: 'login' }" size="lg" class="!w-full !justify-center">
					{{ formatMessage(commonMessages.signInButton) }}
				</ButtonLink>
			</template>

			<template v-else-if="asked">
				<Admonition type="success" :body="formatMessage(messages.onItsWay)" />
				<p class="m-0 text-secondary">
					{{ formatMessage(messages.validFor, { minutes: LINK_MINUTES }) }}
				</p>
				<Button
					size="lg"
					class="!w-full !justify-center"
					:disabled="wait > 0"
					:loading="busy"
					@click="submit"
				>
					<SpinnerIcon v-if="busy" class="animate-spin" />
					{{
						wait > 0
							? formatMessage(messages.againIn, { seconds: wait })
							: formatMessage(messages.again)
					}}
				</Button>
				<ButtonLink :to="{ name: 'login' }" size="lg" class="!w-full !justify-center">
					{{ formatMessage(commonMessages.signInButton) }}
				</ButtonLink>
			</template>

			<form v-else class="flex flex-col gap-4" @submit.prevent="submit">
				<div class="flex flex-col gap-1.5">
					<label class="text-sm font-semibold text-contrast" for="email">
						{{ formatMessage(messages.emailLabel) }}
					</label>
					<StyledInput
						id="email"
						v-model="email"
						type="email"
						name="email"
						autocomplete="email"
						autocapitalize="none"
						autocorrect="off"
						:spellcheck="false"
						:disabled="busy"
						:input-attrs="{ autofocus: true, inputmode: 'email' }"
						wrapper-class="w-full"
					/>
				</div>

				<Admonition v-if="failure" type="critical" :body="failure" />

				<Button
					native-type="submit"
					type="colored"
					color="brand"
					size="lg"
					:disabled="email.trim() === ''"
					:loading="busy"
					class="!w-full !justify-center"
				>
					<SpinnerIcon v-if="busy" class="animate-spin" />
					{{ formatMessage(messages.submit) }}
				</Button>

				<p class="m-0 text-center text-sm text-secondary">
					<RouterLink class="text-link" :to="{ name: 'login' }">
						{{ formatMessage(messages.back) }}
					</RouterLink>
				</p>
			</form>
		</Card>
	</div>
</template>

<script setup lang="ts">
import { SpinnerIcon } from '@modrinth/assets'
import {
	Admonition,
	Button,
	ButtonLink,
	Card,
	commonMessages,
	defineMessages,
	type MessageDescriptor,
	StyledInput,
	useVIntl,
} from '@modrinth/ui'
import { computed, onMounted, onUnmounted, ref } from 'vue'
import { RouterLink } from 'vue-router'

import { isApiRequestError } from '@/api'
import { recovery } from '@/api/recovery'
import { type AuthOptions, registrations } from '@/api/registration'
import { LINK_MINUTES, secondsLeft } from '@/pages/auth/recovery'

const { formatMessage } = useVIntl()

const messages = defineMessages({
	title: { id: 'auth.forgot-password.title', defaultMessage: 'Forgot your password?' },
	subtitle: {
		id: 'auth.forgot-password.subtitle',
		defaultMessage: 'Give us the address on your account and we send a link.',
	},
	loading: { id: 'auth.forgot-password.loading', defaultMessage: 'One moment…' },
	notSetUp: {
		id: 'auth.forgot-password.not-set-up',
		defaultMessage:
			'This panel has no password recovery set up. Ask the administrator to let you back in.',
	},
	emailLabel: { id: 'auth.forgot-password.email', defaultMessage: 'Email address' },
	submit: { id: 'auth.forgot-password.submit', defaultMessage: 'Send the link' },
	back: { id: 'auth.forgot-password.back', defaultMessage: 'Back to signing in' },
	onItsWay: {
		id: 'auth.forgot-password.on-its-way',
		defaultMessage: 'If there is an account for that address, a mail is on its way.',
	},
	validFor: {
		id: 'auth.forgot-password.valid-for',
		defaultMessage:
			'The link works once and for {minutes} minutes. It sometimes lands in the spam folder.',
	},
	again: { id: 'auth.forgot-password.again', defaultMessage: 'Send it again' },
	againIn: { id: 'auth.forgot-password.again-in', defaultMessage: 'Send again in {seconds} s' },
	failed: {
		id: 'auth.forgot-password.failed',
		defaultMessage: 'That did not work. Please try again.',
	},
})

const failures: Record<string, MessageDescriptor> = defineMessages({
	too_many_attempts: {
		id: 'auth.forgot-password.error.too-many-attempts',
		defaultMessage: 'Too many attempts. Wait a few minutes before trying again.',
	},
	network_unreachable: {
		id: 'auth.forgot-password.error.network-unreachable',
		defaultMessage: 'The panel could not be reached.',
	},
})

const options = ref<AuthOptions | null>(null)
const failed = ref(false)
const email = ref('')
const busy = ref(false)
const asked = ref(false)
const failure = ref<string | null>(null)
const askedAt = ref<number | null>(null)
const now = ref(Date.now())

const available = computed(() => options.value?.password_reset_enabled === true)
const wait = computed(() => secondsLeft(askedAt.value, now.value))

let ticking: ReturnType<typeof setInterval> | null = null

onMounted(async () => {
	try {
		options.value = await registrations.options()
	} catch {
		failed.value = true
	}
	ticking = setInterval(() => (now.value = Date.now()), 1000)
})

onUnmounted(() => {
	if (ticking !== null) clearInterval(ticking)
})

async function submit() {
	if (busy.value || wait.value > 0) return
	busy.value = true
	failure.value = null

	try {
		await recovery.request(email.value.trim())
		asked.value = true
		askedAt.value = Date.now()
	} catch (error) {
		const code = isApiRequestError(error) ? error.code : ''
		failure.value = formatMessage(failures[code] ?? messages.failed)
	} finally {
		busy.value = false
	}
}
</script>
