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

			<template v-else-if="!open">
				<Admonition type="info" :body="formatMessage(messages.closed)" />
				<ButtonLink :to="{ name: 'login' }" size="lg" class="!w-full !justify-center">
					{{ formatMessage(commonMessages.signInButton) }}
				</ButtonLink>
			</template>

			<template v-else-if="sent">
				<Admonition type="success" :body="formatMessage(messages.checkYourInbox)" />
				<p class="m-0 text-secondary">{{ formatMessage(messages.spamHint) }}</p>
				<Button
					size="lg"
					class="!w-full !justify-center"
					:disabled="wait > 0"
					:loading="busy"
					@click="resend"
				>
					<SpinnerIcon v-if="busy" class="animate-spin" />
					{{
						wait > 0
							? formatMessage(messages.resendIn, { seconds: wait })
							: formatMessage(messages.resend)
					}}
				</Button>
				<ButtonLink :to="{ name: 'login' }" size="lg" class="!w-full !justify-center">
					{{ formatMessage(commonMessages.signInButton) }}
				</ButtonLink>
			</template>

			<form v-else class="flex flex-col gap-4" @submit.prevent="submit">
				<Admonition
					v-if="approval"
					type="info"
					:body="formatMessage(messages.approvalFollows)"
				/>

				<div class="flex flex-col gap-1.5">
					<label class="text-sm font-semibold text-contrast" for="username">
						{{ formatMessage(commonMessages.usernameLabel) }}
					</label>
					<StyledInput
						id="username"
						v-model="username"
						name="username"
						autocomplete="username"
						autocapitalize="none"
						autocorrect="off"
						:spellcheck="false"
						:disabled="busy"
						:input-attrs="{ autofocus: true }"
						wrapper-class="w-full"
					/>
					<span class="text-sm text-secondary">{{ formatMessage(messages.nameHint) }}</span>
				</div>

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
						:input-attrs="{ inputmode: 'email' }"
						wrapper-class="w-full"
					/>
					<span class="text-sm text-secondary">{{ formatMessage(messages.emailHint) }}</span>
				</div>

				<div class="flex flex-col gap-1.5">
					<label class="text-sm font-semibold text-contrast" for="password">
						{{ formatMessage(commonMessages.passwordLabel) }}
					</label>
					<StyledInput
						id="password"
						v-model="password"
						type="password"
						name="password"
						autocomplete="new-password"
						:disabled="busy"
						wrapper-class="w-full"
					/>
					<span class="text-sm text-secondary">
						{{ formatMessage(messages.passwordHint, { length: PASSWORD_MIN_LENGTH }) }}
					</span>
				</div>

				<Admonition v-if="failure" type="critical" :body="failure" />

				<Button
					native-type="submit"
					type="colored"
					color="brand"
					size="lg"
					:disabled="!ready"
					:loading="busy"
					class="!w-full !justify-center"
				>
					<SpinnerIcon v-if="busy" class="animate-spin" />
					{{ formatMessage(messages.submit) }}
				</Button>

				<p class="m-0 text-center text-sm text-secondary">
					{{ formatMessage(messages.haveAnAccount) }}
					<RouterLink class="text-link" :to="{ name: 'login' }">
						{{ formatMessage(commonMessages.signInButton) }}
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
import { type AuthOptions, registrations } from '@/api/registration'
import {
	approvalFollows,
	formReady,
	PASSWORD_MIN_LENGTH,
	signUpOpen,
} from '@/pages/auth/register'
import { secondsLeft } from '@/pages/auth/recovery'

const { formatMessage } = useVIntl()

const messages = defineMessages({
	title: { id: 'auth.register.title', defaultMessage: 'Create an account' },
	subtitle: {
		id: 'auth.register.subtitle',
		defaultMessage: 'Pick a name, confirm your address, and the panel is yours.',
	},
	loading: { id: 'auth.register.loading', defaultMessage: 'One moment…' },
	closed: {
		id: 'auth.register.closed',
		defaultMessage:
			'This panel is not taking sign-ups at the moment. Ask the administrator for an account.',
	},
	approvalFollows: {
		id: 'auth.register.approval-follows',
		defaultMessage:
			'After you confirm your address an administrator has to let the account in. You will get a mail either way.',
	},
	emailLabel: { id: 'auth.register.email', defaultMessage: 'Email address' },
	emailHint: {
		id: 'auth.register.email-hint',
		defaultMessage: 'You need it to confirm the account and to recover a forgotten password.',
	},
	nameHint: {
		id: 'auth.register.name-hint',
		defaultMessage: '3 to 39 characters: lower case letters, digits, - and _',
	},
	passwordHint: {
		id: 'auth.register.password-hint',
		defaultMessage: 'At least {length} characters.',
	},
	submit: { id: 'auth.register.submit', defaultMessage: 'Create account' },
	haveAnAccount: { id: 'auth.register.have-an-account', defaultMessage: 'Already have one?' },
	checkYourInbox: {
		id: 'auth.register.check-your-inbox',
		defaultMessage: 'Check your inbox: we sent a link that confirms your address.',
	},
	spamHint: {
		id: 'auth.register.spam-hint',
		defaultMessage: 'It can take a minute, and it sometimes lands in the spam folder.',
	},
	resend: { id: 'auth.register.resend', defaultMessage: 'Send the mail again' },
	resendIn: {
		id: 'auth.register.resend-in',
		defaultMessage: 'Send again in {seconds} s',
	},
	failed: { id: 'auth.register.failed', defaultMessage: 'That did not work. Please try again.' },
})

const failures: Record<string, MessageDescriptor> = defineMessages({
	username_taken: {
		id: 'auth.register.error.username-taken',
		defaultMessage: 'That name is taken. Pick another one.',
	},
	invalid_email: {
		id: 'auth.register.error.invalid-email',
		defaultMessage: 'That does not look like an email address.',
	},
	weak_password: {
		id: 'auth.register.error.weak-password',
		defaultMessage: 'That password is too short.',
	},
	invalid_request: {
		id: 'auth.register.error.invalid-request',
		defaultMessage: 'Check the name: 3 to 39 characters, lower case letters, digits, - and _',
	},
	registration_disabled: {
		id: 'auth.register.error.disabled',
		defaultMessage: 'This panel is not taking sign-ups at the moment.',
	},
	rate_limited: {
		id: 'auth.register.error.rate-limited',
		defaultMessage: 'Too many attempts from here. Try again later.',
	},
	network_unreachable: {
		id: 'auth.register.error.network-unreachable',
		defaultMessage: 'The panel could not be reached.',
	},
})

const options = ref<AuthOptions | null>(null)
const failed = ref(false)
const username = ref('')
const email = ref('')
const password = ref('')
const busy = ref(false)
const sent = ref(false)
const failure = ref<string | null>(null)
const askedAt = ref<number | null>(null)
const now = ref(Date.now())

const open = computed(() => signUpOpen(options.value))
const approval = computed(() => approvalFollows(options.value))
const ready = computed(() =>
	formReady({ username: username.value, email: email.value, password: password.value }),
)
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
	if (busy.value || !ready.value) return
	busy.value = true
	failure.value = null

	try {
		await registrations.register({
			username: username.value.trim(),
			email: email.value.trim(),
			password: password.value,
		})
		sent.value = true
		askedAt.value = Date.now()
		password.value = ''
	} catch (error) {
		const code = isApiRequestError(error) ? error.code : ''
		failure.value = formatMessage(failures[code] ?? messages.failed)
	} finally {
		busy.value = false
	}
}

async function resend() {
	if (busy.value || wait.value > 0) return
	busy.value = true
	try {
		await registrations.resendVerification(email.value.trim())
	} catch {
	} finally {
		askedAt.value = Date.now()
		busy.value = false
	}
}
</script>
