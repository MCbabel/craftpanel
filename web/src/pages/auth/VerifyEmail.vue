<template>
	<div class="mx-auto flex min-h-full w-full max-w-[27rem] flex-col justify-center px-6 py-12">
		<Card class="!mb-0 flex flex-col gap-6">
			<div class="flex flex-col gap-1">
				<h1 class="m-0 text-2xl font-extrabold text-contrast">
					{{ formatMessage(messages.title) }}
				</h1>
			</div>

			<div v-if="outcome === null" class="flex items-center gap-2 text-secondary">
				<SpinnerIcon class="animate-spin" />
				{{ formatMessage(messages.checking) }}
			</div>

			<template v-else-if="outcome === 'done'">
				<Admonition type="success" :body="formatMessage(messages.done)" />
				<ButtonLink
					:to="{ name: 'login' }"
					type="colored"
					color="brand"
					size="lg"
					class="!w-full !justify-center"
				>
					{{ formatMessage(commonMessages.signInButton) }}
				</ButtonLink>
			</template>

			<template v-else>
				<Admonition type="critical" :body="formatMessage(reason)" />

				<form v-if="canAskAgain && !askedAgain" class="flex flex-col gap-4" @submit.prevent="askAgain">
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
					</div>
					<Button
						native-type="submit"
						size="lg"
						class="!w-full !justify-center"
						:loading="busy"
						:disabled="email.trim() === ''"
					>
						<SpinnerIcon v-if="busy" class="animate-spin" />
						{{ formatMessage(messages.newMail) }}
					</Button>
				</form>
				<Admonition
					v-else-if="askedAgain"
					type="info"
					:body="formatMessage(messages.newMailSent)"
				/>

				<ButtonLink :to="{ name: 'login' }" size="lg" class="!w-full !justify-center">
					{{ formatMessage(commonMessages.signInButton) }}
				</ButtonLink>
			</template>
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
import { computed, onMounted, ref } from 'vue'
import { useRouter } from 'vue-router'

import { isApiRequestError } from '@/api'
import { registrations } from '@/api/registration'
import {
	canAskForANewMail,
	outcomeOf,
	outcomeOfError,
	tokenFromLocation,
	type VerifyOutcome,
} from '@/pages/auth/register'

const { formatMessage } = useVIntl()
const router = useRouter()

const messages = defineMessages({
	title: { id: 'auth.verify-email.title', defaultMessage: 'Confirm your address' },
	checking: { id: 'auth.verify-email.checking', defaultMessage: 'One moment…' },
	done: {
		id: 'auth.verify-email.done',
		defaultMessage: 'Your account is ready. Sign in and the panel is yours.',
	},
	expired: {
		id: 'auth.verify-email.expired',
		defaultMessage: 'This link has run out. Ask for a new one and check your inbox again.',
	},
	unknown: {
		id: 'auth.verify-email.unknown',
		defaultMessage: 'This link is not valid. If you have already confirmed, just sign in.',
	},
	closed: {
		id: 'auth.verify-email.closed',
		defaultMessage: 'This panel is not taking sign-ups at the moment.',
	},
	missing: {
		id: 'auth.verify-email.missing',
		defaultMessage: 'This page needs the link out of the mail we sent you.',
	},
	failed: {
		id: 'auth.verify-email.failed',
		defaultMessage: 'That did not work. Please try the link again.',
	},
	emailLabel: { id: 'auth.verify-email.email', defaultMessage: 'Your email address' },
	newMail: { id: 'auth.verify-email.new-mail', defaultMessage: 'Send a new link' },
	newMailSent: {
		id: 'auth.verify-email.new-mail-sent',
		defaultMessage:
			'If that address has a sign-up waiting, a new link is on its way. Check your inbox.',
	},
})

const reasons: Record<Exclude<VerifyOutcome, 'done' | 'waiting'>, MessageDescriptor> = {
	expired: messages.expired,
	unknown: messages.unknown,
	closed: messages.closed,
	failed: messages.failed,
}

const outcome = ref<VerifyOutcome | null>(null)
const withoutALink = ref(false)
const email = ref('')
const busy = ref(false)
const askedAgain = ref(false)
let token: string | null = null

const canAskAgain = computed(() => outcome.value !== null && canAskForANewMail(outcome.value))
const reason = computed(() => {
	if (withoutALink.value) return messages.missing
	if (outcome.value === null || outcome.value === 'done' || outcome.value === 'waiting') {
		return messages.failed
	}
	return reasons[outcome.value]
})

onMounted(async () => {
	token = tokenFromLocation({ hash: window.location.hash, search: window.location.search })

	if (token !== null) {
		window.history.replaceState(window.history.state, '', window.location.pathname)
	}

	if (token === null) {
		withoutALink.value = true
		outcome.value = 'failed'
		return
	}

	try {
		const answer = await registrations.verifyEmail(token)
		const state = outcomeOf(answer.state)
		if (state === 'waiting') {
			await router.replace({ name: 'registration-pending' })
			return
		}
		outcome.value = state
	} catch (error) {
		outcome.value = outcomeOfError(isApiRequestError(error) ? error.code : '')
	}
})

async function askAgain() {
	if (busy.value) return
	busy.value = true
	try {
		await registrations.resendVerification(email.value.trim())
	} catch {
	} finally {
		askedAgain.value = true
		busy.value = false
	}
}
</script>
