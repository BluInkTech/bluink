// SPDX-License-Identifier: MPL-2.0
// Copyright (c) 2023–2026 BluInk

import { mkdtemp, readFile, rm, writeFile } from 'node:fs/promises'
import os from 'node:os'
import path from 'node:path'
import { npm, run, writeJson } from './release-utils.mjs'
import { versionPattern } from './release-identity.mjs'

const registryPause = () => new Promise((resolve) => setTimeout(resolve, 5000))

export function provenancePolicy(record) {
	if (
		!['@bluink/cli', '@bluink/fonts', '@bluink/typst'].includes(record.name) ||
		!versionPattern.test(record.version)
	)
		throw new Error('Unknown package provenance policy.')
	const kind = record.name.split('/')[1]
	return {
		workflow: `.github/workflows/${kind === 'cli' ? 'public' : kind}-release.yml`,
		ref: `refs/tags/${kind === 'cli' ? '' : `${kind}/`}v${record.version}`,
	}
}

export async function registryRecord(name, version) {
	const response = await fetch(
		`https://registry.npmjs.org/${encodeURIComponent(name)}/${version}`,
		{ signal: AbortSignal.timeout(30_000) },
	)
	if (response.status === 404) return undefined
	if (!response.ok)
		throw new Error(`Registry verification failed: HTTP ${response.status}`)
	return response.json()
}

function matching(record, existing) {
	if (
		existing &&
		(existing.name !== record.name ||
			existing.version !== record.version ||
			existing.dist?.integrity !== record.integrity)
	)
		throw new Error(
			`npm already has different bytes: ${record.name}@${record.version}`,
		)
}

/** Preflight every version before the first irreversible upload. */
export async function publishRecords(
	records,
	{
		lookup = registryRecord,
		upload = (record) =>
			npm([
				'publish',
				record.archive,
				'--access',
				'public',
				'--tag',
				/-(alpha|beta|rc)$/.exec(record.version)?.[1] ?? 'latest',
				'--provenance',
				'--ignore-scripts',
			]),
		verifyExisting = async () => {},
		pause = registryPause,
		attempts = 121,
	} = {},
) {
	const existing = await Promise.all(
		records.map((record) => lookup(record.name, record.version)),
	)
	for (let index = 0; index < records.length; index++)
		matching(records[index], existing[index])
	await verifyExisting(records.filter((_record, index) => existing[index]))
	for (let index = 0; index < records.length; index++) {
		const record = records[index]
		let uploadError
		if (!existing[index]) {
			try {
				await upload(record)
			} catch (error) {
				uploadError = error
			}
		}
		let verified = false
		for (let attempt = 0; attempt < attempts; attempt++) {
			let published
			try {
				published = await lookup(record.name, record.version)
			} catch (error) {
				if (attempt === attempts - 1) throw error
			}
			matching(record, published)
			if (published?.dist?.attestations) {
				verified = true
				break
			}
			if (attempt < attempts - 1) await pause()
		}
		if (!verified)
			throw new Error(
				`Registry/provenance verification pending: ${record.name}. Reuse the retained archives when retrying.`,
				{ cause: uploadError },
			)
	}
}

/** The version endpoint can propagate before npm's installation catalogue. */
export async function installRegistryPackages(
	directory,
	records,
	{
		install = () =>
			npm(
				[
					'install',
					'--ignore-scripts',
					'--no-audit',
					'--no-fund',
					'--prefer-online',
					'--cache',
					path.join(directory, 'cache'),
					'--registry=https://registry.npmjs.org/',
				],
				{ cwd: directory, capture: true },
			),
		pause = registryPause,
		attempts = 121,
	} = {},
) {
	return retryRegistryMetadata(records, install, {
		pause,
		attempts,
		beforeRetry: () =>
			rm(path.join(directory, 'package-lock.json'), { force: true }),
	})
}

async function retryRegistryMetadata(
	records,
	operation,
	{ pause, attempts, beforeRetry = async () => {} },
) {
	for (let attempt = 0; attempt < attempts; attempt++) {
		try {
			return await operation()
		} catch (error) {
			if (
				!error.message.includes('npm error code ETARGET') ||
				!records.some((record) =>
					error.message.includes(
						`No matching version found for ${record.name}@${record.version}.`,
					),
				) ||
				attempt === attempts - 1
			)
				throw error
			console.log(
				'Waiting for the published version to reach the npm catalogue…',
			)
			await beforeRetry()
			await pause()
		}
	}
}

export async function auditRegistrySignatures(
	directory,
	records,
	{
		audit = () =>
			npm(
				[
					'audit',
					'signatures',
					'--json',
					'--include-attestations',
					'--prefer-online',
					'--cache',
					path.join(directory, 'cache'),
					'--registry=https://registry.npmjs.org/',
				],
				{ cwd: directory, capture: true },
			),
		pause = registryPause,
		attempts = 121,
	} = {},
) {
	return JSON.parse(
		await retryRegistryMetadata(records, audit, { pause, attempts }),
	)
}

/** Apply identity policy only to bundles already verified by npm audit. */
export function provenanceStatement(report, record, identity) {
	const policy = provenancePolicy(record)
	if (
		!Array.isArray(report.invalid) ||
		report.invalid.length ||
		!Array.isArray(report.missing) ||
		report.missing.length
	)
		throw new Error('npm signature verification did not pass.')
	const verified = report.verified?.find(
		(entry) => entry.name === record.name && entry.version === record.version,
	)
	const attestation = verified?.attestationBundles?.find(
		(entry) => entry.predicateType === 'https://slsa.dev/provenance/v1',
	)
	if (!attestation)
		throw new Error(`Missing verified provenance: ${record.name}`)
	const statement = JSON.parse(
		Buffer.from(attestation.bundle.dsseEnvelope.payload, 'base64').toString(
			'utf8',
		),
	)
	const definition = statement.predicate?.buildDefinition
	const workflow = definition?.externalParameters?.workflow
	const dependency = definition?.resolvedDependencies?.find(
		(entry) =>
			entry.uri === `git+${identity.publicRepository}@${workflow?.ref}`,
	)
	const digest = Buffer.from(
		record.integrity.slice('sha512-'.length),
		'base64',
	).toString('hex')
	if (
		statement.predicateType !== attestation.predicateType ||
		statement.subject?.length !== 1 ||
		statement.subject[0].name !==
			`pkg:npm/${record.name.replace('@', '%40')}@${record.version}` ||
		statement.subject[0].digest?.sha512 !== digest ||
		workflow?.repository !== identity.publicRepository ||
		workflow?.path !== policy.workflow ||
		workflow?.ref !== policy.ref ||
		!/^[a-f0-9]{40}$/.test(dependency?.digest?.gitCommit) ||
		((record.name === '@bluink/cli' || identity.packageName === record.name) &&
			dependency.digest.gitCommit !== identity.publicCommit)
	)
		throw new Error(
			`Registry provenance has the wrong workflow, source or subject: ${record.name}`,
		)
	return {
		bundle: attestation.bundle,
		ref: workflow.ref,
		commit: dependency.digest.gitCommit,
		workflow: policy.workflow,
	}
}

/** npm verifies registry/Sigstore signatures; gh additionally pins certificate identity. */
export async function verifyRegistryProvenance(records, identity) {
	if (!records.length) return []
	const directory = await mkdtemp(
		path.join(os.tmpdir(), 'bluink-registry-verification-'),
	)
	try {
		await writeJson(path.join(directory, 'package.json'), {
			private: true,
			dependencies: Object.fromEntries(
				records.map((record) => [record.name, record.version]),
			),
		})
		await installRegistryPackages(directory, records)
		const lock = JSON.parse(
			await readFile(path.join(directory, 'package-lock.json'), 'utf8'),
		)
		for (const record of records)
			if (
				lock.packages[`node_modules/${record.name}`]?.integrity !==
				record.integrity
			)
				throw new Error(
					`Registry installation differs from prepared bytes: ${record.name}`,
				)
		const report = await auditRegistrySignatures(directory, records)
		for (const record of records.filter(
			(entry) => entry.name !== '@bluink/web',
		)) {
			const verified = provenanceStatement(report, record, identity)
			const bundle = path.join(directory, 'provenance.jsonl')
			await writeFile(bundle, `${JSON.stringify(verified.bundle)}\n`)
			await run('gh', [
				'attestation',
				'verify',
				record.archive,
				'--digest-alg',
				'sha512',
				'--bundle',
				bundle,
				'--repo',
				identity.publicRepository.slice('https://github.com/'.length),
				'--signer-workflow',
				`${identity.publicRepository.slice('https://github.com/'.length)}/${verified.workflow}`,
				'--source-ref',
				verified.ref,
				'--source-digest',
				verified.commit,
				'--deny-self-hosted-runners',
			])
		}
		return records
			.filter((entry) => entry.name !== '@bluink/web')
			.map(({ name, version, integrity }) => ({
				name,
				version,
				integrity,
				status: 'verified',
			}))
	} finally {
		await rm(directory, { recursive: true, force: true })
	}
}
