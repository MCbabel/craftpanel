<template>
	<div class="flex flex-col gap-6">
		<div class="flex flex-wrap items-center justify-between gap-3">
			<div class="flex flex-col gap-1">
				<h1 class="m-0 text-2xl font-extrabold text-contrast">
					{{ formatMessage(messages.title) }}
				</h1>
				<p class="m-0 text-secondary">{{ formatMessage(messages.subtitle) }}</p>
			</div>
			<Button
				v-tooltip="canManage ? undefined : formatMessage(commonMessages.noPermissionAction)"
				type="colored"
				color="brand"
				:disabled="!canManage || host === null"
				@click="openCreate"
			>
				<UserPlusIcon aria-hidden="true" />
				{{ formatMessage(messages.createUser) }}
			</Button>
		</div>

		<Card v-if="host" class="!mb-0 flex flex-col gap-4">
			<SettingsLabel
				:title="formatMessage(messages.hostTitle)"
				:description="formatMessage(messages.hostDescription)"
			/>

			<Admonition
				v-if="overbooked"
				type="warning"
				:header="formatMessage(messages.overbookedHeader)"
				:body="
					formatMessage(messages.overbookedBody, {
						allocated: formatNumber(host.allocated.memory_mib),
						assignable: formatNumber(host.assignable_memory_mib),
					})
				"
			/>

			<div class="grid gap-4 sm:grid-cols-2 xl:grid-cols-4">
				<div class="flex flex-col gap-1.5 rounded-xl bg-surface-2 p-4">
					<span class="flex items-center gap-1.5 text-sm font-semibold text-secondary">
						<MemoryStickIcon aria-hidden="true" class="size-4" />
						{{ formatMessage(messages.hostMemory) }}
					</span>
					<span class="text-lg font-extrabold text-contrast">
						{{
							formatMessage(messages.ofMib, {
								value: formatNumber(host.allocated.memory_mib),
								total: formatNumber(host.assignable_memory_mib),
							})
						}}
					</span>
					<ProgressBar
						:progress="Math.min(host.allocated.memory_mib, host.assignable_memory_mib)"
						:max="Math.max(host.assignable_memory_mib, 1)"
						:color="overbooked ? 'red' : 'brand'"
						full-width
					/>
					<span class="text-xs text-secondary">
						{{
							formatMessage(messages.hostMemoryUsed, {
								used: formatBytes(host.used.memory_bytes),
								total: formatBytes(host.memory_total_bytes),
							})
						}}
					</span>
				</div>

				<div class="flex flex-col gap-1.5 rounded-xl bg-surface-2 p-4">
					<span class="flex items-center gap-1.5 text-sm font-semibold text-secondary">
						<CpuIcon aria-hidden="true" class="size-4" />
						{{ formatMessage(messages.hostCpu) }}
					</span>
					<span class="text-lg font-extrabold text-contrast">
						{{
							formatMessage(messages.ofCores, {
								value: formatNumber(host.allocated.cpu_cores),
								total: formatNumber(host.cpu_cores),
							})
						}}
					</span>
					<span class="text-xs text-secondary">
						{{
							shareUsers === 0
								? formatMessage(messages.hostCpuAllCapped)
								: formatMessage(messages.hostCpuMixed, { count: formatNumber(shareUsers) })
						}}
					</span>
					<span class="text-xs text-secondary">
						{{ formatMessage(messages.hostCpuUsed, { used: formatNumber(usedCores) }) }}
					</span>
				</div>

				<div class="flex flex-col gap-1.5 rounded-xl bg-surface-2 p-4">
					<span class="flex items-center gap-1.5 text-sm font-semibold text-secondary">
						<GaugeIcon aria-hidden="true" class="size-4" />
						{{ formatMessage(messages.hostProcesses) }}
					</span>
					<span class="text-lg font-extrabold text-contrast">
						{{ formatNumber(host.used.pids) }}
					</span>
					<span class="text-xs text-secondary">
						{{ formatMessage(messages.hostProcessesHint) }}
					</span>
				</div>

				<div class="flex flex-col gap-1.5 rounded-xl bg-surface-2 p-4">
					<span class="flex items-center gap-1.5 text-sm font-semibold text-secondary">
						<UsersIcon aria-hidden="true" class="size-4" />
						{{ formatMessage(messages.hostUsers) }}
					</span>
					<span class="text-lg font-extrabold text-contrast">
						{{ formatNumber(host.user_count) }}
					</span>
					<span class="text-xs text-secondary">
						{{ formatMessage(messages.measuredAt, { time: formatTime(host.measured_at) }) }}
					</span>
				</div>
			</div>
		</Card>

		<div class="flex flex-wrap items-center gap-3">
			<StyledInput
				v-model="search"
				:icon="SearchIcon"
				:placeholder="formatMessage(messages.searchPlaceholder)"
				autocapitalize="none"
				autocorrect="off"
				:spellcheck="false"
				clearable
				wrapper-class="w-full sm:w-80"
			/>
			<span v-if="!loading && !failure" class="text-sm text-secondary">
				{{ formatMessage(messages.userCount, { count: formatNumber(total) }) }}
			</span>
		</div>

		<Admonition
			v-if="actionFailure"
			type="critical"
			:body="actionFailure"
			dismissible
			@dismiss="actionFailure = null"
		/>

		<LoadingIndicator v-if="loading" />

		<Admonition
			v-else-if="failure"
			type="critical"
			:header="formatMessage(messages.loadFailed)"
			:body="failure"
		>
			<template #actions>
				<Button @click="load()">
					<UpdatedIcon aria-hidden="true" />
					{{ formatMessage(commonMessages.retryButton) }}
				</Button>
			</template>
		</Admonition>

		<template v-else>
			<Table
				:columns="columns"
				:data="rows"
				row-key="id"
				:table-min-width="wide ? '72rem' : undefined"
				:row-below-visible="!wide"
			>
				<template #empty-state>
					<EmptyState
						:type="search ? 'no-search-result' : 'empty'"
						:heading="formatMessage(search ? messages.noMatches : messages.noUsers)"
						:description="formatMessage(search ? messages.noMatchesHint : messages.noUsersHint)"
					>
						<template v-if="!search && canManage" #actions>
							<Button type="colored" color="brand" @click="openCreate">
								<UserPlusIcon aria-hidden="true" />
								{{ formatMessage(messages.createUser) }}
							</Button>
						</template>
					</EmptyState>
				</template>

				<template #cell-user="{ index }">
					<div class="flex min-w-0 items-center gap-2">
						<Avatar
							:src="at(index).avatar_url"
							:tint-by="at(index).username"
							size="28px"
							circle
							no-shadow
						/>
						<div class="flex min-w-0 flex-col">
							<span class="truncate font-medium text-contrast">{{ at(index).username }}</span>
							<span v-if="at(index).id === me?.id" class="text-xs text-secondary">
								{{ formatMessage(messages.you) }}
							</span>
						</div>
					</div>
				</template>

				<template #cell-role="{ index }">
					<Badge v-if="at(index).panel_role === 'admin'" type="admin" />
					<Badge v-else :type="formatMessage(messages.roleUser)" color="gray" />
				</template>

				<template #cell-system="{ index }">
					<div class="flex items-center gap-1.5">
						<span v-tooltip="systemTooltip(index)">
							<Badge :type="systemLabel(index)" :color="systemColor(index)" />
						</span>
						<IconButton
							v-if="at(index).system_user.state !== 'ready'"
							v-tooltip="
								canManage
									? formatMessage(messages.retrySystemUser)
									: formatMessage(commonMessages.noPermissionAction)
							"
							type="quiet"
							size="sm"
							:label="formatMessage(messages.retrySystemUser)"
							:disabled="!canManage"
							:loading="retryingId === at(index).id"
							@click="retrySystemUser(at(index))"
						>
							<SpinnerIcon v-if="retryingId === at(index).id" class="animate-spin" />
							<UpdatedIcon v-else aria-hidden="true" />
						</IconButton>
					</div>
				</template>

				<template #cell-memory="{ index }">
					<div class="flex min-w-0 flex-col gap-1">
						<div class="flex items-center gap-2">
							<span
								class="text-sm font-medium"
								:class="overLimit(index) ? 'text-red' : 'text-contrast'"
							>
								{{ memoryLabel(index) }}
							</span>
							<span v-if="overLimit(index)" v-tooltip="formatMessage(messages.overLimitHint)">
								<Badge :type="formatMessage(messages.overLimit)" color="red" />
							</span>
						</div>
						<ProgressBar
							v-if="hasMemoryLimit(index)"
							:progress="memoryProgress(index)"
							:max="memoryCeiling(index)"
							:color="overLimit(index) ? 'red' : 'brand'"
							full-width
						/>
						<span class="text-xs text-secondary">{{ diskLabel(index) }}</span>
					</div>
				</template>

				<template #cell-usage="{ index }">
					<div class="flex min-w-0 flex-col">
						<span class="text-sm text-contrast">{{ memoryUsedLabel(index) }}</span>
						<span class="text-xs text-secondary">{{ cpuAndPidsLabel(index) }}</span>
					</div>
				</template>

				<template #cell-servers="{ index }">
					<div class="flex min-w-0 flex-col">
						<span class="text-sm text-contrast">
							{{ formatNumber(at(index).usage.servers.total) }}
						</span>
						<span class="text-xs text-secondary">{{ runningLabel(index) }}</span>
					</div>
				</template>

				<template #cell-actions="{ index }">
					<div class="flex items-center justify-end">
						<TeleportOverflowMenu
							type="quiet"
							:label="formatMessage(messages.rowActions, { username: at(index).username })"
							:options="rowActions(at(index))"
						>
							<MoreVerticalIcon aria-hidden="true" class="size-5" />
						</TeleportOverflowMenu>
					</div>
				</template>

				<template #row-below="{ index }">
					<dl
						class="m-0 grid grid-cols-[auto_1fr] items-center gap-x-4 gap-y-2 px-4 pb-4 text-sm"
					>
						<dt class="text-secondary">{{ formatMessage(messages.columnRole) }}</dt>
						<dd class="m-0">
							<Badge v-if="at(index).panel_role === 'admin'" type="admin" />
							<Badge v-else :type="formatMessage(messages.roleUser)" color="gray" />
						</dd>

						<dt class="text-secondary">{{ formatMessage(messages.columnSystem) }}</dt>
						<dd class="m-0 flex items-center gap-1.5">
							<span v-tooltip="systemTooltip(index)">
								<Badge :type="systemLabel(index)" :color="systemColor(index)" />
							</span>
							<IconButton
								v-if="at(index).system_user.state !== 'ready'"
								type="quiet"
								size="sm"
								:label="formatMessage(messages.retrySystemUser)"
								:disabled="!canManage"
								:loading="retryingId === at(index).id"
								@click="retrySystemUser(at(index))"
							>
								<SpinnerIcon v-if="retryingId === at(index).id" class="animate-spin" />
								<UpdatedIcon v-else aria-hidden="true" />
							</IconButton>
						</dd>

						<dt class="text-secondary">{{ formatMessage(messages.columnMemory) }}</dt>
						<dd class="m-0 flex min-w-0 flex-col gap-1">
							<div class="flex items-center gap-2">
								<span
									class="font-medium"
									:class="overLimit(index) ? 'text-red' : 'text-contrast'"
								>
									{{ memoryLabel(index) }}
								</span>
								<span v-if="overLimit(index)" v-tooltip="formatMessage(messages.overLimitHint)">
									<Badge :type="formatMessage(messages.overLimit)" color="red" />
								</span>
							</div>
							<ProgressBar
								v-if="hasMemoryLimit(index)"
								:progress="memoryProgress(index)"
								:max="memoryCeiling(index)"
								:color="overLimit(index) ? 'red' : 'brand'"
								full-width
							/>
							<span class="text-xs text-secondary">{{ diskLabel(index) }}</span>
						</dd>

						<dt class="text-secondary">{{ formatMessage(messages.columnUsage) }}</dt>
						<dd class="m-0 flex min-w-0 flex-col">
							<span class="text-contrast">{{ memoryUsedLabel(index) }}</span>
							<span class="text-xs text-secondary">{{ cpuAndPidsLabel(index) }}</span>
						</dd>

						<dt class="text-secondary">{{ formatMessage(messages.columnServers) }}</dt>
						<dd class="m-0 flex min-w-0 flex-col">
							<span class="text-contrast">
								{{ formatNumber(at(index).usage.servers.total) }}
							</span>
							<span class="text-xs text-secondary">{{ runningLabel(index) }}</span>
						</dd>
					</dl>
				</template>
			</Table>

			<div v-if="pageCount > 1" class="flex justify-center">
				<Pagination :page="page" :count="pageCount" @switch-page="switchPage" />
			</div>
		</template>

		<NewModal ref="createModal" :header="formatMessage(messages.createUser)" width="34rem">
			<form class="flex flex-col gap-5" @submit.prevent="submitCreate">
				<div class="flex flex-col gap-1.5">
					<label class="text-sm font-semibold text-contrast" for="create-username">
						{{ formatMessage(commonMessages.usernameLabel) }}
					</label>
					<StyledInput
						id="create-username"
						v-model="createUsername"
						autocapitalize="none"
						autocorrect="off"
						:spellcheck="false"
						:disabled="createBusy"
						:error="createUsername.length > 0 && !createUsernameValid"
						wrapper-class="w-full"
					/>
					<span class="text-xs text-secondary">{{ formatMessage(messages.usernameRule) }}</span>
				</div>

				<div class="flex flex-col gap-1.5">
					<label class="text-sm font-semibold text-contrast" for="create-email">
						{{ formatMessage(messages.emailLabel) }}
					</label>
					<StyledInput
						id="create-email"
						v-model="createEmail"
						type="email"
						autocapitalize="none"
						autocorrect="off"
						:spellcheck="false"
						:disabled="createBusy"
						:error="!createEmailValid"
						:input-attrs="{ inputmode: 'email' }"
						wrapper-class="w-full"
					/>
					<span class="text-xs text-secondary">{{ formatMessage(messages.emailHint) }}</span>
				</div>

				<div class="flex flex-col gap-1.5">
					<span class="text-sm font-semibold text-contrast">
						{{ formatMessage(messages.roleLabel) }}
					</span>
					<DropdownSelect
						:model-value="createRole"
						:options="ROLES"
						:display-name="roleName"
						name="create-role"
						:disabled="createBusy"
						class="!h-9 !w-full"
						@update:model-value="(value: unknown) => (createRole = roleFrom(value, createRole))"
					/>
				</div>

				<div class="flex items-center justify-between gap-4">
					<label class="flex flex-col gap-0.5" for="create-must-change">
						<span class="font-semibold text-contrast">
							{{ formatMessage(messages.mustChangeTitle) }}
						</span>
						<span class="text-sm text-secondary">
							{{ formatMessage(messages.mustChangeDescription) }}
						</span>
					</label>
					<Toggle id="create-must-change" v-model="createMustChange" :disabled="createBusy" />
				</div>

				<template v-if="createRole === 'user'">
					<div class="flex items-center justify-between gap-4">
						<label class="flex flex-col gap-0.5" for="create-default-limits">
							<span class="font-semibold text-contrast">
								{{ formatMessage(messages.defaultLimitsTitle) }}
							</span>
							<span class="text-sm text-secondary">
								{{ formatMessage(messages.defaultLimitsDescription) }}
							</span>
						</label>
						<Toggle id="create-default-limits" v-model="createUseDefaults" :disabled="createBusy" />
					</div>

					<UserLimitFields
						v-if="!createUseDefaults && host"
						v-model="createLimits"
						scope="create"
						:memory-max="host.assignable_memory_mib"
						:disk-max="host.assignable_disk_mib"
						:disabled="createBusy"
					/>
				</template>
				<Admonition
					v-else
					type="info"
					:body="formatMessage(messages.adminHasNoLimits)"
				/>

				<Admonition type="info" :body="formatMessage(messages.passwordGenerated)" />
				<Admonition v-if="createFailure" type="critical" :body="createFailure" />
			</form>

			<template #actions>
				<div class="flex justify-end gap-2">
					<Button :disabled="createBusy" @click="createModal?.hide()">
						<XIcon aria-hidden="true" />
						{{ formatMessage(commonMessages.cancelButton) }}
					</Button>
					<Button
						type="colored"
						color="brand"
						:disabled="!createUsernameValid || !createEmailValid || createBusy"
						@click="submitCreate"
					>
						<SpinnerIcon v-if="createBusy" class="animate-spin" />
						<UserPlusIcon v-else aria-hidden="true" />
						{{ formatMessage(messages.createUser) }}
					</Button>
				</div>
			</template>
		</NewModal>

		<NewModal ref="editModal" :header="formatMessage(messages.editUser)" width="34rem">
			<form v-if="editing" class="flex flex-col gap-5" @submit.prevent="submitEdit">
				<div class="flex flex-col gap-1.5">
					<label class="text-sm font-semibold text-contrast" for="edit-username">
						{{ formatMessage(commonMessages.usernameLabel) }}
					</label>
					<StyledInput
						id="edit-username"
						v-model="editUsername"
						autocapitalize="none"
						autocorrect="off"
						:spellcheck="false"
						:disabled="editLocked"
						:error="!editUsernameValid"
						wrapper-class="w-full"
					/>
					<span class="text-xs text-secondary">{{ formatMessage(messages.usernameRule) }}</span>
				</div>

				<div class="flex flex-col gap-1.5">
					<label class="text-sm font-semibold text-contrast" for="edit-email">
						{{ formatMessage(messages.emailLabel) }}
					</label>
					<StyledInput
						id="edit-email"
						v-model="editEmail"
						type="email"
						autocapitalize="none"
						autocorrect="off"
						:spellcheck="false"
						:disabled="editLocked"
						:error="!editEmailValid"
						:input-attrs="{ inputmode: 'email' }"
						wrapper-class="w-full"
					/>
					<Admonition
						v-if="emailBeingRemoved"
						type="warning"
						:body="formatMessage(messages.emailRemoveWarning)"
					/>
					<span v-else class="text-xs text-secondary">
						{{ formatMessage(messages.emailHint) }}
					</span>
				</div>

				<div class="flex flex-col gap-1.5">
					<span class="text-sm font-semibold text-contrast">
						{{ formatMessage(messages.roleLabel) }}
					</span>
					<DropdownSelect
						:model-value="editRole"
						:options="ROLES"
						:display-name="roleName"
						name="edit-role"
						:disabled="editLocked"
						class="!h-9 !w-full"
						@update:model-value="(value: unknown) => (editRole = roleFrom(value, editRole))"
					/>
				</div>

				<div class="flex items-center justify-between gap-4">
					<label class="flex flex-col gap-0.5" for="edit-must-change">
						<span class="font-semibold text-contrast">
							{{ formatMessage(messages.mustChangeTitle) }}
						</span>
						<span class="text-sm text-secondary">
							{{ formatMessage(messages.mustChangeDescription) }}
						</span>
					</label>
					<Toggle id="edit-must-change" v-model="editMustChange" :disabled="editLocked" />
				</div>

				<div class="flex flex-col gap-2 rounded-xl bg-surface-2 p-4">
					<SettingsLabel
						:title="formatMessage(messages.resetPasswordTitle)"
						:description="
							formatMessage(
								editing.id === me?.id
									? messages.resetPasswordSelf
									: messages.resetPasswordDescription,
							)
						"
					/>
					<ButtonLink
						v-if="editing.id === me?.id"
						:to="{ name: 'change-password' }"
						class="w-fit"
					>
						<KeyIcon aria-hidden="true" />
						{{ formatMessage(messages.changeOwnPassword) }}
					</ButtonLink>
					<template v-else>
						<div class="flex flex-wrap gap-2">
							<Button
								class="w-fit"
								:disabled="editLocked || linkProblem !== null"
								@click="sendResetLink"
							>
								<SpinnerIcon v-if="linkBusy" class="animate-spin" />
								<SendIcon v-else aria-hidden="true" />
								{{ formatMessage(messages.sendLinkAction) }}
							</Button>
							<Button
								type="outlined"
								color="orange"
								class="w-fit"
								:disabled="editLocked"
								@click="resetPassword"
							>
								<SpinnerIcon v-if="resetBusy" class="animate-spin" />
								<KeyIcon v-else aria-hidden="true" />
								{{ formatMessage(messages.resetPasswordAction) }}
							</Button>
						</div>

						<Admonition
							v-if="linkSent"
							type="success"
							:body="formatMessage(messages.linkSent, { email: editing.email ?? '' })"
						/>
						<p v-else-if="linkProblem === null" class="m-0 text-xs text-secondary">
							{{
								formatMessage(messages.linkGoesTo, {
									email: editing.email ?? '',
									minutes: LINK_MINUTES,
								})
							}}
						</p>
						<p v-else class="m-0 text-xs text-secondary">
							{{ formatMessage(LINK_PROBLEMS[linkProblem]) }}
							<RouterLink
								v-if="linkProblem === 'no-mail' || linkProblem === 'no-link-base'"
								class="text-link hover:underline"
								:to="{ name: 'admin-mail' }"
							>
								{{ formatMessage(messages.linkSetUpMail) }}
							</RouterLink>
						</p>
					</template>
				</div>

				<div class="flex flex-col gap-1 text-sm text-secondary">
					<span>
						{{ formatMessage(messages.createdAt, { time: formatDate(editing.created_at) }) }}
					</span>
					<span>
						{{
							formatMessage(messages.lastLogin, {
								time: editing.last_login_at
									? formatDate(editing.last_login_at)
									: formatMessage(messages.never),
							})
						}}
					</span>
				</div>

				<Admonition v-if="editFailure" type="critical" :body="editFailure" />
			</form>

			<template #actions>
				<div class="flex justify-end gap-2">
					<Button :disabled="editLocked" @click="editModal?.hide()">
						<XIcon aria-hidden="true" />
						{{ formatMessage(commonMessages.cancelButton) }}
					</Button>
					<Button
						type="colored"
						:color="emailBeingRemoved ? 'orange' : 'brand'"
						:disabled="!editUsernameValid || !editEmailValid || editLocked"
						@click="submitEdit"
					>
						<SpinnerIcon v-if="editBusy" class="animate-spin" />
						<SaveIcon v-else aria-hidden="true" />
						{{
							formatMessage(
								emailBeingRemoved ? messages.saveAndRemoveEmail : commonMessages.saveButton,
							)
						}}
					</Button>
				</div>
			</template>
		</NewModal>

		<NewModal ref="limitsModal" :header="limitsHeader" width="38rem">
			<LoadingIndicator v-if="limitsLoading" />
			<Admonition
				v-else-if="limitsLoadFailure"
				type="critical"
				:header="formatMessage(messages.loadFailed)"
				:body="limitsLoadFailure"
			>
				<template #actions>
					<Button @click="loadLimits()">
						<UpdatedIcon aria-hidden="true" />
						{{ formatMessage(commonMessages.retryButton) }}
					</Button>
				</template>
			</Admonition>
			<div v-else-if="limitsDraft && limitsHost" class="flex flex-col gap-5">
				<UserLimitFields
					v-model="limitsDraft"
					scope="limits"
					:memory-max="limitsHost.assignable_memory_mib"
					:disk-max="limitsHost.assignable_disk_mib"
					:usage="limitsUsage"
					:disabled="limitsBusy"
				/>
				<Admonition v-if="limitsFailure" type="critical" :body="limitsFailure" />
			</div>

			<template #actions>
				<div class="flex justify-end gap-2">
					<Button :disabled="limitsBusy" @click="limitsModal?.hide()">
						<XIcon aria-hidden="true" />
						{{ formatMessage(commonMessages.cancelButton) }}
					</Button>
					<Button
						type="colored"
						color="brand"
						:disabled="limitsDraft === null || limitsBusy || !canManage"
						@click="submitLimits"
					>
						<SpinnerIcon v-if="limitsBusy" class="animate-spin" />
						<SaveIcon v-else aria-hidden="true" />
						{{ formatMessage(commonMessages.saveButton) }}
					</Button>
				</div>
			</template>
		</NewModal>

		<NewModal
			ref="deleteModal"
			fade="danger"
			:header="formatMessage(messages.deleteUser)"
			width="36rem"
		>
			<LoadingIndicator v-if="deleteLoading" />
			<Admonition
				v-else-if="deleteLoadFailure"
				type="critical"
				:header="formatMessage(messages.loadFailed)"
				:body="deleteLoadFailure"
			>
				<template #actions>
					<Button @click="loadDeleteDetail()">
						<UpdatedIcon aria-hidden="true" />
						{{ formatMessage(commonMessages.retryButton) }}
					</Button>
				</template>
			</Admonition>
			<div v-else-if="deleting && deleteDetail" class="flex flex-col gap-4">
				<p class="m-0">
					{{ formatMessage(messages.deleteIntro, { username: deleting.username }) }}
				</p>

				<Admonition
					v-if="deleteRunning"
					type="critical"
					:header="formatMessage(messages.deleteRunningHeader)"
					:body="formatMessage(messages.deleteRunningBody)"
				/>

				<template v-if="deleteDetail.owned_servers.length > 0">
					<div class="flex flex-col gap-1 rounded-xl bg-surface-2 p-4">
						<span class="text-sm font-semibold text-contrast">
							{{ formatMessage(messages.ownedServers) }}
						</span>
						<span
							v-for="owned in deleteDetail.owned_servers"
							:key="owned.id"
							class="flex items-center justify-between gap-3 text-sm text-secondary"
						>
							<span class="truncate">{{ owned.name }}</span>
							<span class="shrink-0">
								{{ formatNumber(owned.memory_mib) }} MiB
								<template v-if="owned.running">
									· {{ formatMessage(messages.runningNow) }}
								</template>
							</span>
						</span>
					</div>

					<div class="flex flex-col gap-2">
						<SettingsLabel
							:title="formatMessage(messages.serversChoiceTitle)"
							:description="formatMessage(messages.serversChoiceDescription)"
						/>
						<RadioButtons v-model="deleteChoice" :items="DELETE_CHOICES">
							<template #default="{ item }">
								<span>
									{{
										formatMessage(
											item === 'transfer' ? messages.choiceTransfer : messages.choiceDelete,
										)
									}}
								</span>
							</template>
						</RadioButtons>
					</div>

					<div v-if="deleteChoice === 'transfer'" class="flex flex-col gap-1.5">
						<span class="text-sm font-semibold text-contrast">
							{{ formatMessage(messages.transferTarget) }}
						</span>
						<DropdownSelect
							:model-value="transferName"
							:options="transferNames"
							:placeholder="formatMessage(messages.transferPlaceholder)"
							name="transfer-target"
							:disabled="deleteBusy || transferNames.length === 0"
							class="!h-9 !w-full"
							@update:model-value="setTransferName"
						/>
						<span v-if="transferNames.length === 0" class="text-xs text-red">
							{{ formatMessage(messages.noTransferTarget) }}
						</span>
						<span v-else class="text-xs text-secondary">
							{{ formatMessage(messages.transferHint) }}
						</span>
					</div>
				</template>

				<div class="flex flex-col gap-1.5">
					<label class="text-sm font-semibold text-contrast" for="delete-confirm">
						{{ formatMessage(messages.typeToConfirm, { username: deleting.username }) }}
					</label>
					<StyledInput
						id="delete-confirm"
						v-model="deleteConfirm"
						autocapitalize="none"
						autocorrect="off"
						:spellcheck="false"
						:disabled="deleteBusy"
						wrapper-class="w-full sm:w-72"
					/>
				</div>

				<Admonition v-if="deleteFailure" type="critical" :body="deleteFailure" />
			</div>

			<template #actions>
				<div class="flex justify-end gap-2">
					<Button :disabled="deleteBusy" @click="deleteModal?.hide()">
						<XIcon aria-hidden="true" />
						{{ formatMessage(commonMessages.cancelButton) }}
					</Button>
					<Button
						type="colored"
						color="red"
						:disabled="!deleteReady"
						@click="submitDelete"
					>
						<SpinnerIcon v-if="deleteBusy" class="animate-spin" />
						<TrashIcon v-else aria-hidden="true" />
						{{ formatMessage(messages.deleteUser) }}
					</Button>
				</div>
			</template>
		</NewModal>

		<NewModal
			ref="passwordModal"
			:header="formatMessage(messages.passwordHeader)"
			:on-after-hide="forgetPassword"
			width="30rem"
		>
			<div class="flex flex-col gap-4">
				<Admonition type="warning" :body="formatMessage(messages.passwordOnce)" />
				<div class="flex flex-col gap-1.5">
					<span class="text-sm font-semibold text-contrast">{{ revealedFor }}</span>
					<CopyCode :text="revealedPassword" />
				</div>
			</div>
			<template #actions>
				<div class="flex justify-end">
					<Button type="colored" color="brand" @click="passwordModal?.hide()">
						<CheckIcon aria-hidden="true" />
						{{ formatMessage(messages.passwordDone) }}
					</Button>
				</div>
			</template>
		</NewModal>
	</div>
</template>

<script setup lang="ts">
import {
	CheckIcon,
	CpuIcon,
	EditIcon,
	GaugeIcon,
	KeyIcon,
	MemoryStickIcon,
	MoreVerticalIcon,
	SaveIcon,
	SearchIcon,
	SendIcon,
	SpinnerIcon,
	TrashIcon,
	UpdatedIcon,
	UserPlusIcon,
	UsersIcon,
	XIcon,
} from '@modrinth/assets'
import {
	Admonition,
	Avatar,
	Badge,
	Button,
	ButtonLink,
	Card,
	commonMessages,
	CopyCode,
	defineMessages,
	DropdownSelect,
	EmptyState,
	IconButton,
	LoadingIndicator,
	type MessageDescriptor,
	NewModal,
	type OverflowMenuOption,
	Pagination,
	ProgressBar,
	RadioButtons,
	SettingsLabel,
	StyledInput,
	type TableColumn,
	Table,
	TeleportOverflowMenu,
	Toggle,
	useFormatBytes,
	useFormatDateTime,
	useFormatNumber,
	useVIntl,
} from '@modrinth/ui'
import { watchDebounced } from '@vueuse/core'
import { computed, onMounted, ref, watch } from 'vue'
import { RouterLink } from 'vue-router'

import {
	type AdminUserDetail,
	api,
	type DeleteUserQuery,
	type DeleteUserServers,
	type HostCapacity,
	isApiRequestError,
	type PanelRole,
	type PanelUser,
	type Ulid,
	type UserLimits,
	type UserUsage,
} from '@/api'
import { mail, type MailSettings } from '@/api/mail'
import { recovery } from '@/api/recovery'
import { actionsColumnWidth, ICON_BUTTON_REM } from '@/components/table-widths'
import { useWideScreen } from '@/composables/breakpoint'
import { useSession } from '@/composables/session'
import UserLimitFields from '@/pages/admin/UserLimitFields.vue'
import {
	addressFromField,
	type ResetLinkProblem,
	resetLinkProblem,
	sameAddress,
} from '@/pages/admin/users'
import { LINK_MINUTES } from '@/pages/auth/recovery'
import { addressLooksRight } from '@/pages/auth/register'

const { formatMessage } = useVIntl()
const formatBytes = useFormatBytes()
const formatNumber = useFormatNumber()
const formatDate = useFormatDateTime({ dateStyle: 'medium', timeStyle: 'short' })
const formatTime = useFormatDateTime({ timeStyle: 'medium' })
const { user: me, refresh: refreshSession } = useSession()

const PAGE_SIZE = 25
const ROLES: PanelRole[] = ['user', 'admin']
const DELETE_CHOICES: DeleteUserServers[] = ['transfer', 'delete']
const USERNAME_PATTERN = /^[a-z0-9_-]{3,39}$/
const PASSWORD_ALPHABET = 'abcdefghijkmnopqrstuvwxyzABCDEFGHJKLMNPQRSTUVWXYZ23456789'

const messages = defineMessages({
	title: { id: 'admin.users.title', defaultMessage: 'Users' },
	subtitle: {
		id: 'admin.users.subtitle',
		defaultMessage: 'Every account on this machine, what it may use, and what it uses.',
	},
	createUser: { id: 'admin.users.create', defaultMessage: 'Create user' },
	editUser: { id: 'admin.users.edit', defaultMessage: 'Edit account' },
	deleteUser: { id: 'admin.users.delete', defaultMessage: 'Delete user' },
	editLimits: { id: 'admin.users.edit-limits', defaultMessage: 'Limits' },
	hostTitle: { id: 'admin.users.host.title', defaultMessage: 'This machine' },
	hostDescription: {
		id: 'admin.users.host.description',
		defaultMessage: 'What the machine has, and how much of it has already been handed out.',
	},
	hostMemory: { id: 'admin.users.host.memory', defaultMessage: 'Memory handed out' },
	hostMemoryUsed: {
		id: 'admin.users.host.memory-used',
		defaultMessage: '{used} of {total} in use',
	},
	hostCpu: { id: 'admin.users.host.cpu', defaultMessage: 'Cores handed out' },
	hostCpuAllCapped: {
		id: 'admin.users.host.cpu-all-capped',
		defaultMessage: 'Every account is on a hard cap, so this sum is a real ceiling.',
	},
	hostCpuMixed: {
		id: 'admin.users.host.cpu-mixed',
		defaultMessage: 'Includes {count} account(s) on a share, which have no ceiling; the sum is only an indication.',
	},
	hostCpuUsed: { id: 'admin.users.host.cpu-used', defaultMessage: '{used} cores in use' },
	hostProcesses: { id: 'admin.users.host.processes', defaultMessage: 'Processes running' },
	hostProcessesHint: {
		id: 'admin.users.host.processes-hint',
		defaultMessage: 'Across every account.',
	},
	hostUsers: { id: 'admin.users.host.users', defaultMessage: 'Accounts' },
	measuredAt: { id: 'admin.users.host.measured', defaultMessage: 'Measured at {time}' },
	overbookedHeader: {
		id: 'admin.users.overbooked.header',
		defaultMessage: 'This machine is overbooked',
	},
	overbookedBody: {
		id: 'admin.users.overbooked.body',
		defaultMessage:
			'{allocated} MiB is promised to accounts, but only {assignable} MiB is assignable. That is allowed — nothing is refused — but if everyone uses their share at once, the machine will not keep up.',
	},
	ofMib: { id: 'admin.users.of-mib', defaultMessage: '{value} of {total} MiB' },
	ofCores: { id: 'admin.users.of-cores', defaultMessage: '{value} of {total} cores' },
	searchPlaceholder: { id: 'admin.users.search', defaultMessage: 'Search by username...' },
	userCount: { id: 'admin.users.count', defaultMessage: '{count} account(s)' },
	loadFailed: { id: 'admin.users.load-failed', defaultMessage: 'Could not load' },
	noUsers: { id: 'admin.users.empty', defaultMessage: 'No accounts yet' },
	noUsersHint: {
		id: 'admin.users.empty-hint',
		defaultMessage: 'Create the first account to hand out part of this machine.',
	},
	noMatches: { id: 'admin.users.no-matches', defaultMessage: 'No account matches' },
	noMatchesHint: {
		id: 'admin.users.no-matches-hint',
		defaultMessage: 'Try a different username.',
	},
	columnUser: { id: 'admin.users.column.user', defaultMessage: 'User' },
	columnRole: { id: 'admin.users.column.role', defaultMessage: 'Role' },
	columnSystem: { id: 'admin.users.column.system', defaultMessage: 'System account' },
	columnMemory: { id: 'admin.users.column.memory', defaultMessage: 'Memory handed out' },
	columnUsage: { id: 'admin.users.column.usage', defaultMessage: 'In use' },
	columnServers: { id: 'admin.users.column.servers', defaultMessage: 'Servers' },
	columnActions: { id: 'admin.users.column.actions', defaultMessage: 'Actions' },
	you: { id: 'admin.users.you', defaultMessage: 'That is you' },
	roleLabel: { id: 'admin.users.role', defaultMessage: 'Role' },
	roleUser: { id: 'admin.users.role.user', defaultMessage: 'User' },
	roleAdmin: { id: 'admin.users.role.admin', defaultMessage: 'Admin' },
	systemReady: { id: 'admin.users.system.ready', defaultMessage: 'Ready' },
	systemProvisioning: { id: 'admin.users.system.provisioning', defaultMessage: 'Provisioning' },
	systemError: { id: 'admin.users.system.error', defaultMessage: 'Error' },
	retrySystemUser: {
		id: 'admin.users.system.retry',
		defaultMessage: 'Create the system account again',
	},
	overLimit: { id: 'admin.users.over-limit', defaultMessage: 'Over limit' },
	overLimitHint: {
		id: 'admin.users.over-limit.hint',
		defaultMessage:
			'More memory is handed out to servers than the limit allows. Nothing is killed, but this account cannot create or start a server until a server is deleted or shrunk.',
	},
	systemNotReadyHint: {
		id: 'admin.users.system.not-ready-hint',
		defaultMessage: 'Without a system account this user can sign in but cannot create servers.',
	},
	cpuModeCap: { id: 'admin.limits.cpu.mode.cap', defaultMessage: 'Hard cap' },
	cpuModeShare: { id: 'admin.limits.cpu.mode.share', defaultMessage: 'Share' },
	inUse: { id: 'admin.users.in-use', defaultMessage: '{used} memory' },
	mibNoLimit: { id: 'admin.users.mib-no-limit', defaultMessage: '{value} MiB, no limit' },
	adminHasNoLimits: {
		id: 'admin.users.admin-has-no-limits',
		defaultMessage:
			'An administrator has no limits: no memory ceiling, no processor cap, no process count and no disk quota. There is nothing to set here.',
	},
	diskOf: { id: 'admin.users.disk-of', defaultMessage: '{used} of {total} MiB on disk' },
	diskNoLimit: { id: 'admin.users.disk-no-limit', defaultMessage: '{used} on disk, no limit' },
	cpuAndPidsNoLimit: {
		id: 'admin.users.cpu-and-pids-no-limit',
		defaultMessage: '{used} cores · {pids} processes · no limits',
	},
	cpuAndPids: {
		id: 'admin.users.cpu-and-pids',
		defaultMessage: '{used} of {limit} cores ({mode}) · {pids} of {pidsLimit} processes',
	},
	running: { id: 'admin.users.running', defaultMessage: '{count} running' },
	runningNow: { id: 'admin.users.running-now', defaultMessage: 'running' },
	rowActions: { id: 'admin.users.row-actions', defaultMessage: 'Actions for {username}' },
	usernameRule: {
		id: 'admin.users.username-rule',
		defaultMessage: '3 to 39 characters, lowercase letters, digits, hyphen and underscore.',
	},
	emailLabel: { id: 'admin.users.email', defaultMessage: 'Email address (optional)' },
	emailHint: {
		id: 'admin.users.email-hint',
		defaultMessage:
			'The only way this account can recover a forgotten password itself. No confirmation mail goes out: an address an administrator types counts as usable.',
	},
	emailRemoveWarning: {
		id: 'admin.users.email-remove-warning',
		defaultMessage:
			'Saving now takes the address off the account. It then has no way back over mail: a new password, shown once, is all that is left for it.',
	},
	saveAndRemoveEmail: {
		id: 'admin.users.save-and-remove-email',
		defaultMessage: 'Save and remove the address',
	},
	mustChangeTitle: {
		id: 'admin.users.must-change.title',
		defaultMessage: 'Change password at next sign-in',
	},
	mustChangeDescription: {
		id: 'admin.users.must-change.description',
		defaultMessage: 'The account cannot go anywhere else until the password is changed.',
	},
	defaultLimitsTitle: {
		id: 'admin.users.default-limits.title',
		defaultMessage: 'Use the panel defaults',
	},
	defaultLimitsDescription: {
		id: 'admin.users.default-limits.description',
		defaultMessage: 'Turn this off to set memory, processor and processes for this account.',
	},
	passwordGenerated: {
		id: 'admin.users.password-generated',
		defaultMessage: 'A password is generated for the account and shown once, right after creation.',
	},
	passwordHeader: { id: 'admin.users.password.header', defaultMessage: 'The new password' },
	passwordOnce: {
		id: 'admin.users.password.once',
		defaultMessage:
			'This is the only time this password is shown. Copy it and pass it on now — the panel cannot show it again.',
	},
	passwordDone: { id: 'admin.users.password.done', defaultMessage: 'I have copied it' },
	resetPasswordTitle: { id: 'admin.users.reset-password.title', defaultMessage: 'Password' },
	resetPasswordDescription: {
		id: 'admin.users.reset-password.description',
		defaultMessage:
			'Either a link by mail, after which nobody but the owner knows the password — or a new password shown here once, which ends every session of the account immediately.',
	},
	resetPasswordAction: {
		id: 'admin.users.reset-password.action',
		defaultMessage: 'Generate a new password',
	},
	resetPasswordSelf: {
		id: 'admin.users.reset-password.self',
		defaultMessage:
			'Setting a password ends every session of the account — for your own account that includes this one, and the new password would be gone before you could read it.',
	},
	changeOwnPassword: {
		id: 'admin.users.reset-password.change-own',
		defaultMessage: 'Change your own password',
	},
	sendLinkAction: { id: 'admin.users.send-link.action', defaultMessage: 'Send a reset link' },
	linkGoesTo: {
		id: 'admin.users.send-link.goes-to',
		defaultMessage:
			'A link goes to {email}. It works once and for {minutes} minutes, and no session ends until a new password is set.',
	},
	linkSent: {
		id: 'admin.users.send-link.sent',
		defaultMessage: 'A link is on its way to {email}. Older links of this account stopped working.',
	},
	linkNoAddress: {
		id: 'admin.users.send-link.no-address',
		defaultMessage:
			'There is no address on this account, so no link can go anywhere — a generated password is the only way for it.',
	},
	linkNoMail: {
		id: 'admin.users.send-link.no-mail',
		defaultMessage: 'This panel cannot send mail yet, so no link can go out.',
	},
	linkNoLinkBase: {
		id: 'admin.users.send-link.no-link-base',
		defaultMessage:
			'Mail works, but the panel does not know its own address, so a link cannot be built.',
	},
	linkUnsavedAddress: {
		id: 'admin.users.send-link.unsaved-address',
		defaultMessage:
			'Save first: the link goes to the address on the account, not to what is typed above.',
	},
	linkSetUpMail: { id: 'admin.users.send-link.set-up-mail', defaultMessage: 'Set up mail' },
	createdAt: { id: 'admin.users.created-at', defaultMessage: 'Created {time}' },
	lastLogin: { id: 'admin.users.last-login', defaultMessage: 'Last signed in {time}' },
	never: { id: 'admin.users.never', defaultMessage: 'never' },
	limitsHeader: { id: 'admin.users.limits.header', defaultMessage: 'Limits for {username}' },
	deleteIntro: {
		id: 'admin.users.delete.intro',
		defaultMessage:
			'{username} and their system account will be removed. This cannot be undone.',
	},
	deleteRunningHeader: {
		id: 'admin.users.delete.running-header',
		defaultMessage: 'A server of this account is running',
	},
	deleteRunningBody: {
		id: 'admin.users.delete.running-body',
		defaultMessage: 'Stop every server of this account first. The panel does not kill them for you.',
	},
	ownedServers: { id: 'admin.users.delete.owned', defaultMessage: 'Servers of this account' },
	serversChoiceTitle: {
		id: 'admin.users.delete.choice-title',
		defaultMessage: 'What happens to these servers',
	},
	serversChoiceDescription: {
		id: 'admin.users.delete.choice-description',
		defaultMessage: 'Either they move to another account, or they go with the account.',
	},
	choiceTransfer: {
		id: 'admin.users.delete.choice-transfer',
		defaultMessage: 'Transfer them to another account',
	},
	choiceDelete: {
		id: 'admin.users.delete.choice-delete',
		defaultMessage: 'Delete them, with their files and backups',
	},
	transferTarget: { id: 'admin.users.delete.transfer-target', defaultMessage: 'New owner' },
	transferPlaceholder: {
		id: 'admin.users.delete.transfer-placeholder',
		defaultMessage: 'Pick an account',
	},
	transferHint: {
		id: 'admin.users.delete.transfer-hint',
		defaultMessage:
			'If this pushes the new owner over their limit, the transfer still happens; they then count as over limit.',
	},
	noTransferTarget: {
		id: 'admin.users.delete.no-transfer-target',
		defaultMessage: 'There is no other account to transfer to.',
	},
	typeToConfirm: {
		id: 'admin.users.delete.type-to-confirm',
		defaultMessage: 'Type {username} to confirm',
	},
	unknownError: {
		id: 'admin.users.unknown-error',
		defaultMessage: 'Something went wrong. Try again.',
	},
})

const LINK_PROBLEMS: Record<ResetLinkProblem, MessageDescriptor> = {
	'no-address': messages.linkNoAddress,
	'no-mail': messages.linkNoMail,
	'no-link-base': messages.linkNoLinkBase,
	'unsaved-address': messages.linkUnsavedAddress,
}

const linkFailures: Record<string, MessageDescriptor> = defineMessages({
	user_not_found: {
		id: 'admin.users.send-link.error.user-not-found',
		defaultMessage: 'This account is gone — somebody removed it in the meantime.',
	},
	no_email_address: {
		id: 'admin.users.send-link.error.no-email-address',
		defaultMessage: 'There is no address on this account, so nothing could be sent.',
	},
	mail_not_configured: {
		id: 'admin.users.send-link.error.mail-not-configured',
		defaultMessage:
			'Mail is not set up on this panel. Put a key in under Administration → Mail, or generate a password here.',
	},
})

const addressFailures: Record<string, MessageDescriptor> = defineMessages({
	email_taken: {
		id: 'admin.users.error.email-taken',
		defaultMessage:
			'That address is already on another account or on an open sign-up. An open sign-up is somebody waiting for a decision. Approve or reject it under Administration → Sign-ups.',
	},
	invalid_email: {
		id: 'admin.users.error.invalid-email',
		defaultMessage: 'That does not look like an email address.',
	},
})

type UserRow = { id: Ulid; user: PanelUser }
type UserColumn = 'user' | 'role' | 'system' | 'memory' | 'usage' | 'servers' | 'actions'

const users = ref<PanelUser[]>([])
const total = ref(0)
const host = ref<HostCapacity | null>(null)
const mailSetup = ref<MailSettings | null>(null)
const loading = ref(true)
const failure = ref<string | null>(null)
const actionFailure = ref<string | null>(null)
const search = ref('')
const page = ref(1)
const retryingId = ref<Ulid | null>(null)

const canManage = computed(() => me.value?.capabilities.can_manage_panel_users === true)
const wide = useWideScreen()
const rows = computed<UserRow[]>(() => users.value.map((user) => ({ id: user.id, user })))
const pageCount = computed(() => Math.max(1, Math.ceil(total.value / PAGE_SIZE)))
const overbooked = computed(
	() => host.value !== null && host.value.allocated.memory_mib > host.value.assignable_memory_mib,
)
const shareUsers = computed(
	() => users.value.filter((user) => user.limits?.cpu_mode === 'share').length,
)
const usedCores = computed(() => Math.round((host.value?.used.cpu_cores ?? 0) * 100) / 100)

const columns = computed<TableColumn<UserColumn>[]>(() =>
	wide.value
		? [
				{ key: 'user', label: formatMessage(messages.columnUser), width: '18rem' },
				{ key: 'role', label: formatMessage(messages.columnRole), width: '7rem' },
				{ key: 'system', label: formatMessage(messages.columnSystem), width: '11rem' },
				{ key: 'memory', label: formatMessage(messages.columnMemory), width: '14rem' },
				{ key: 'usage', label: formatMessage(messages.columnUsage), width: '16rem' },
				{ key: 'servers', label: formatMessage(messages.columnServers), width: '6rem' },
				{
					key: 'actions',
					label: formatMessage(messages.columnActions),
					align: 'right',
					width: '4rem',
				},
			]
		: [
				{ key: 'user', label: formatMessage(messages.columnUser) },
				{ key: 'actions', align: 'right', width: actionsColumnWidth([ICON_BUTTON_REM]) },
			],
)

let listToken = 0

async function load(): Promise<void> {
	const token = ++listToken
	loading.value = true
	failure.value = null
	try {
		const [list, capacity] = await Promise.all([
			api.admin.users({
				query: search.value.trim() || undefined,
				limit: PAGE_SIZE,
				offset: (page.value - 1) * PAGE_SIZE,
			}),
			api.admin.host(),
		])
		if (token !== listToken) return
		users.value = list.users
		total.value = list.total
		host.value = capacity
		const last = Math.max(1, Math.ceil(list.total / PAGE_SIZE))
		if (list.users.length === 0 && page.value > 1 && last < page.value) {
			page.value = last
			return await load()
		}
	} catch (error) {
		if (token !== listToken) return
		failure.value = reason(error)
	} finally {
		if (token === listToken) loading.value = false
	}
}

onMounted(load)

onMounted(async () => {
	try {
		mailSetup.value = await mail.settings()
	} catch {
	}
})

watchDebounced(
	search,
	() => {
		page.value = 1
		void load()
	},
	{ debounce: 300 },
)

function switchPage(next: number): void {
	page.value = next
	void load()
}

function reason(error: unknown): string {
	return isApiRequestError(error) ? error.message : formatMessage(messages.unknownError)
}

function addressReason(error: unknown): string {
	const known = isApiRequestError(error) ? addressFailures[error.code] : undefined
	return known ? formatMessage(known) : reason(error)
}

async function syncSelf(userId: Ulid): Promise<void> {
	if (userId === me.value?.id) await refreshSession()
}

function at(index: number): PanelUser {
	return users.value[index]
}

function systemLabel(index: number): string {
	const state = at(index).system_user.state
	if (state === 'ready') return formatMessage(messages.systemReady)
	return formatMessage(state === 'error' ? messages.systemError : messages.systemProvisioning)
}

function systemColor(index: number): string {
	const state = at(index).system_user.state
	if (state === 'ready') return 'green'
	return state === 'error' ? 'red' : 'orange'
}

function systemTooltip(index: number): string | undefined {
	const system = at(index).system_user
	if (system.state === 'ready') return undefined
	const hint = formatMessage(messages.systemNotReadyHint)
	return system.error_message ? `${system.error_message}\n${hint}` : hint
}

function overLimit(index: number): boolean {
	return at(index).usage.over_limit
}

function hasMemoryLimit(index: number): boolean {
	return at(index).usage.memory.limit_mib !== null
}

function memoryProgress(index: number): number {
	const memory = at(index).usage.memory
	return Math.min(memory.allocated_mib, memory.limit_mib ?? memory.allocated_mib)
}

function memoryCeiling(index: number): number {
	return Math.max(at(index).usage.memory.limit_mib ?? 1, 1)
}

function memoryLabel(index: number): string {
	const memory = at(index).usage.memory
	if (memory.limit_mib === null) {
		return formatMessage(messages.mibNoLimit, { value: formatNumber(memory.allocated_mib) })
	}
	return formatMessage(messages.ofMib, {
		value: formatNumber(memory.allocated_mib),
		total: formatNumber(memory.limit_mib),
	})
}

function memoryUsedLabel(index: number): string {
	return formatMessage(messages.inUse, {
		used: formatBytes(at(index).usage.memory.used_bytes),
	})
}

function cpuAndPidsLabel(index: number): string {
	const user = at(index)
	const cores = formatNumber(Math.round(user.usage.cpu.used_cores * 100) / 100)
	if (user.limits === null) {
		return formatMessage(messages.cpuAndPidsNoLimit, {
			used: cores,
			pids: formatNumber(user.usage.pids.used),
		})
	}
	return formatMessage(messages.cpuAndPids, {
		used: cores,
		limit: formatNumber(user.usage.cpu.limit_cores ?? user.limits.cpu_cores),
		mode: modeLabel(user.limits.cpu_mode),
		pids: formatNumber(user.usage.pids.used),
		pidsLimit: formatNumber(user.usage.pids.limit ?? user.limits.pids_max),
	})
}

function diskLabel(index: number): string {
	const disk = at(index).usage.disk
	if (disk.limit_mib === null) {
		return formatMessage(messages.diskNoLimit, { used: formatBytes(disk.used_bytes) })
	}
	return formatMessage(messages.diskOf, {
		used: formatBytes(disk.used_bytes),
		total: formatNumber(disk.limit_mib),
	})
}

function runningLabel(index: number): string {
	return formatMessage(messages.running, {
		count: formatNumber(at(index).usage.servers.running),
	})
}

function roleName(role: PanelRole): string {
	return formatMessage(role === 'admin' ? messages.roleAdmin : messages.roleUser)
}

function modeLabel(mode: UserLimits['cpu_mode']): string {
	return formatMessage(mode === 'cap' ? messages.cpuModeCap : messages.cpuModeShare)
}

function roleFrom(value: unknown, fallback: PanelRole): PanelRole {
	return value === 'admin' || value === 'user' ? value : fallback
}

function generatePassword(length = 20): string {
	const bytes = new Uint8Array(length)
	const usable = 256 - (256 % PASSWORD_ALPHABET.length)
	let password = ''
	while (password.length < length) {
		crypto.getRandomValues(bytes)
		for (const byte of bytes) {
			if (byte >= usable) continue
			password += PASSWORD_ALPHABET[byte % PASSWORD_ALPHABET.length]
			if (password.length === length) break
		}
	}
	return password
}

const passwordModal = ref<InstanceType<typeof NewModal> | null>(null)
const revealedFor = ref('')
const revealedPassword = ref('')

function revealPassword(username: string, password: string): void {
	revealedFor.value = username
	revealedPassword.value = password
	passwordModal.value?.show()
}

function forgetPassword(): void {
	revealedFor.value = ''
	revealedPassword.value = ''
}

function rowActions(user: PanelUser): OverflowMenuOption[] {
	const denied = formatMessage(commonMessages.noPermissionAction)
	return [
		{
			id: 'edit',
			label: formatMessage(messages.editUser),
			icon: EditIcon,
			disabled: !canManage.value,
			tooltip: canManage.value ? undefined : denied,
			action: () => openEdit(user),
		},
		{
			id: 'limits',
			label: formatMessage(messages.editLimits),
			icon: GaugeIcon,
			shown: user.panel_role !== 'admin',
			disabled: !canManage.value,
			tooltip: canManage.value ? undefined : denied,
			action: () => openLimits(user),
		},
		{
			id: 'retry-system-user',
			label: formatMessage(messages.retrySystemUser),
			icon: UpdatedIcon,
			shown: user.system_user.state !== 'ready',
			disabled: !canManage.value,
			tooltip: canManage.value ? undefined : denied,
			action: () => void retrySystemUser(user),
		},
		{ type: 'divider', shown: user.id !== me.value?.id },
		{
			id: 'delete',
			label: formatMessage(messages.deleteUser),
			icon: TrashIcon,
			tone: 'red',
			hoverFilled: true,
			shown: user.id !== me.value?.id,
			disabled: !canManage.value,
			tooltip: canManage.value ? undefined : denied,
			action: () => openDelete(user),
		},
	]
}

async function retrySystemUser(user: PanelUser): Promise<void> {
	if (!canManage.value || retryingId.value !== null) return
	retryingId.value = user.id
	actionFailure.value = null
	try {
		await api.admin.retrySystemUser(user.id)
	} catch (error) {
		actionFailure.value = reason(error)
	} finally {
		retryingId.value = null
	}
	await Promise.all([load(), syncSelf(user.id)])
}

const createModal = ref<InstanceType<typeof NewModal> | null>(null)
const createUsername = ref('')
const createEmail = ref('')
const createRole = ref<PanelRole>('user')
const createMustChange = ref(true)
const createUseDefaults = ref(true)
const createLimits = ref<UserLimits>(fallbackLimits())
const createBusy = ref(false)
const createFailure = ref<string | null>(null)

const createUsernameValid = computed(() => USERNAME_PATTERN.test(createUsername.value))
const createEmailValid = computed(() => addressValid(createEmail.value))

function addressValid(typed: string): boolean {
	const address = addressFromField(typed)
	return address === null || addressLooksRight(address)
}

function fallbackLimits(): UserLimits {
	return { memory_mib: 2048, cpu_mode: 'share', cpu_cores: 2, pids_max: 512, disk_mib: 51200 }
}

watch(createRole, () => {
	createUseDefaults.value = true
	createLimits.value = host.value ? { ...host.value.default_limits } : fallbackLimits()
})

function openCreate(): void {
	createUsername.value = ''
	createEmail.value = ''
	createRole.value = 'user'
	createMustChange.value = true
	createUseDefaults.value = true
	createLimits.value = host.value ? { ...host.value.default_limits } : fallbackLimits()
	createFailure.value = null
	createModal.value?.show()
}

async function submitCreate(): Promise<void> {
	if (createBusy.value || !createUsernameValid.value || !createEmailValid.value) return
	createBusy.value = true
	createFailure.value = null
	const password = generatePassword()
	try {
		const created = await api.admin.createUser({
			username: createUsername.value,
			password,
			panel_role: createRole.value,
			email: addressFromField(createEmail.value) ?? undefined,
			must_change_password: createMustChange.value,
			limits:
				createRole.value === 'admin' || createUseDefaults.value ? undefined : createLimits.value,
		})
		createModal.value?.hide()
		revealPassword(created.username, password)
		await load()
	} catch (error) {
		createFailure.value = addressReason(error)
	} finally {
		createBusy.value = false
	}
}

const editModal = ref<InstanceType<typeof NewModal> | null>(null)
const editing = ref<PanelUser | null>(null)
const editUsername = ref('')
const editEmail = ref('')
const editRole = ref<PanelRole>('user')
const editMustChange = ref(false)
const editBusy = ref(false)
const resetBusy = ref(false)
const linkBusy = ref(false)
const linkSent = ref(false)
const editFailure = ref<string | null>(null)

const editUsernameValid = computed(() => USERNAME_PATTERN.test(editUsername.value))
const editEmailValid = computed(() => addressValid(editEmail.value))
const editLocked = computed(() => editBusy.value || resetBusy.value || linkBusy.value)
const emailBeingRemoved = computed(
	() =>
		(editing.value?.email ?? null) !== null && addressFromField(editEmail.value) === null,
)
const linkProblem = computed(() =>
	editing.value === null
		? null
		: resetLinkProblem(editing.value, mailSetup.value, editEmail.value),
)

function openEdit(user: PanelUser): void {
	editing.value = user
	editUsername.value = user.username
	editEmail.value = user.email ?? ''
	editRole.value = user.panel_role
	editMustChange.value = user.must_change_password
	editFailure.value = null
	linkSent.value = false
	editModal.value?.show()
}

async function submitEdit(): Promise<void> {
	const user = editing.value
	if (user === null || editLocked.value || !editUsernameValid.value || !editEmailValid.value) return
	editBusy.value = true
	editFailure.value = null
	try {
		await api.admin.updateUser(user.id, {
			username: editUsername.value === user.username ? undefined : editUsername.value,
			panel_role: editRole.value === user.panel_role ? undefined : editRole.value,
			email: sameAddress(editEmail.value, user.email)
				? undefined
				: addressFromField(editEmail.value),
			must_change_password:
				editMustChange.value === user.must_change_password ? undefined : editMustChange.value,
		})
		editModal.value?.hide()
		await Promise.all([load(), syncSelf(user.id)])
	} catch (error) {
		editFailure.value = addressReason(error)
	} finally {
		editBusy.value = false
	}
}

async function resetPassword(): Promise<void> {
	const user = editing.value
	if (user === null || editLocked.value) return
	resetBusy.value = true
	editFailure.value = null
	const password = generatePassword()
	try {
		await api.admin.updateUser(user.id, { password })
		editModal.value?.hide()
		revealPassword(user.username, password)
		await load()
	} catch (error) {
		editFailure.value = reason(error)
	} finally {
		resetBusy.value = false
	}
}

async function sendResetLink(): Promise<void> {
	const user = editing.value
	if (user === null || editLocked.value || linkProblem.value !== null) return
	linkBusy.value = true
	editFailure.value = null
	linkSent.value = false
	try {
		await recovery.sendFor(user.id)
		linkSent.value = true
	} catch (error) {
		const code = isApiRequestError(error) ? error.code : ''
		const known = linkFailures[code]
		editFailure.value = known ? formatMessage(known) : reason(error)
		if (code === 'user_not_found') await load()
	} finally {
		linkBusy.value = false
	}
}

const limitsModal = ref<InstanceType<typeof NewModal> | null>(null)
const limitsUser = ref<PanelUser | null>(null)
const limitsDraft = ref<UserLimits | null>(null)
const limitsUsage = ref<UserUsage | null>(null)
const limitsHost = ref<{
	cpu_cores: number
	assignable_memory_mib: number
	assignable_disk_mib: number
} | null>(null)
const limitsLoading = ref(false)
const limitsBusy = ref(false)
const limitsFailure = ref<string | null>(null)
const limitsLoadFailure = ref<string | null>(null)

const limitsHeader = computed(() =>
	formatMessage(messages.limitsHeader, { username: limitsUser.value?.username ?? '' }),
)

function openLimits(user: PanelUser): void {
	limitsUser.value = user
	limitsDraft.value = null
	limitsUsage.value = null
	limitsHost.value = null
	limitsFailure.value = null
	limitsLoading.value = true
	limitsModal.value?.show()
	void loadLimits()
}

async function loadLimits(): Promise<void> {
	const user = limitsUser.value
	if (user === null) return
	limitsLoading.value = true
	limitsLoadFailure.value = null
	try {
		const response = await api.admin.limits(user.id)
		limitsDraft.value = response.limits === null ? null : { ...response.limits }
		limitsUsage.value = response.usage
		limitsHost.value = response.host
	} catch (error) {
		limitsLoadFailure.value = reason(error)
	} finally {
		limitsLoading.value = false
	}
}

async function submitLimits(): Promise<void> {
	const user = limitsUser.value
	const draft = limitsDraft.value
	if (user === null || draft === null || limitsBusy.value) return
	limitsBusy.value = true
	limitsFailure.value = null
	try {
		await api.admin.setLimits(user.id, draft)
		limitsModal.value?.hide()
		await Promise.all([load(), syncSelf(user.id)])
	} catch (error) {
		limitsFailure.value = reason(error)
	} finally {
		limitsBusy.value = false
	}
}

const deleteModal = ref<InstanceType<typeof NewModal> | null>(null)
const deleting = ref<PanelUser | null>(null)
const deleteDetail = ref<AdminUserDetail | null>(null)
const deleteChoice = ref<DeleteUserServers>('transfer')
const transferName = ref<string | null>(null)
const transferCandidates = ref<PanelUser[]>([])
const deleteConfirm = ref('')
const deleteLoading = ref(false)
const deleteBusy = ref(false)
const deleteFailure = ref<string | null>(null)
const deleteLoadFailure = ref<string | null>(null)

const deleteRunning = computed(
	() => deleteDetail.value?.owned_servers.some((owned) => owned.running) === true,
)
const transferNames = computed(() => transferCandidates.value.map((user) => user.username))
const deleteReady = computed(() => {
	const detail = deleteDetail.value
	const user = deleting.value
	if (detail === null || user === null || deleteBusy.value || !canManage.value) return false
	if (deleteRunning.value) return false
	if (deleteConfirm.value !== user.username) return false
	if (detail.owned_servers.length === 0) return true
	return deleteChoice.value === 'delete' || transferName.value !== null
})

function openDelete(user: PanelUser): void {
	deleting.value = user
	deleteDetail.value = null
	deleteChoice.value = 'transfer'
	transferName.value = null
	transferCandidates.value = []
	deleteConfirm.value = ''
	deleteFailure.value = null
	deleteLoading.value = true
	deleteModal.value?.show()
	void loadDeleteDetail()
}

async function loadDeleteDetail(): Promise<void> {
	const user = deleting.value
	if (user === null) return
	deleteLoading.value = true
	deleteLoadFailure.value = null
	try {
		const [detail, candidates] = await Promise.all([
			api.admin.user(user.id),
			api.admin.users({ limit: 200 }),
		])
		deleteDetail.value = detail
		transferCandidates.value = candidates.users.filter((other) => other.id !== user.id)
		if (transferCandidates.value.length === 0) deleteChoice.value = 'delete'
	} catch (error) {
		deleteLoadFailure.value = reason(error)
	} finally {
		deleteLoading.value = false
	}
}

function setTransferName(value: unknown): void {
	if (typeof value === 'string') transferName.value = value
}

async function submitDelete(): Promise<void> {
	const user = deleting.value
	const detail = deleteDetail.value
	if (user === null || detail === null || !deleteReady.value) return
	deleteBusy.value = true
	deleteFailure.value = null
	try {
		await api.admin.deleteUser(user.id, deleteQuery(detail))
		deleteModal.value?.hide()
		await load()
	} catch (error) {
		deleteFailure.value = reason(error)
	} finally {
		deleteBusy.value = false
	}
}

function deleteQuery(detail: AdminUserDetail): DeleteUserQuery {
	if (detail.owned_servers.length === 0) return {}
	if (deleteChoice.value === 'delete') return { servers: 'delete' }
	const target = transferCandidates.value.find((user) => user.username === transferName.value)
	return { servers: 'transfer', transfer_to: target?.id }
}
</script>
