// SPDX-License-Identifier: MPL-2.0
// Copyright (c) 2023–2026 BluInk

import { copyFile, mkdir, readFile } from 'node:fs/promises'
import path from 'node:path'
import { fileURLToPath } from 'node:url'
import { parseArgs } from 'node:util'
import { buildPackage } from './assemble-cli.mjs'
import {
	downloadNativeComponents,
	readNativeComponents,
	validateNativeSelection,
} from './native-components.mjs'
import { contractHash, prepareCliBuild } from './cli-build.mjs'
import { downloadFrontend, validateFrontend } from './frontend-asset.mjs'
import {
	inspectArchive,
	packDirectory,
	productionLock,
	verifyArchive,
} from './package-artifacts.mjs'
import { cliFileAllowed } from './package-audit.mjs'
import {
	releaseNativeEntry,
	verifyPlatformArchive,
} from './platform-package.mjs'
import {
	nativePackageVersion,
	publicIdentity,
	versionPattern,
} from './release-identity.mjs'
import {
	publishRecords,
	verifyRegistryProvenance,
	verifyRegistryTag,
} from './registry-publication.mjs'
import {
	fileHash,
	json,
	nativeDirectory,
	nativeHostTarget,
	packageLauncher,
	packageName,
	root,
	run,
	targets,
	verifyNativeArtifacts,
	writeJson,
} from './release-utils.mjs'
import {
	downloadResources,
	readPreparedResources,
	resourceAudit,
	resourceSummary,
	stageInstalledResources,
} from './resource-packages.mjs'
import { smokePackage } from './smoke-package.mjs'

export function validateInput(input) {
	if (
		input?.format !== 'bluink.release-input' ||
		input.formatVersion !== 1 ||
		!versionPattern.test(input.version) ||
		!input.publicRepository ||
		!Array.isArray(input.dependencies) ||
		Object.keys(input).some(
			(key) =>
				![
					'format',
					'formatVersion',
					'version',
					'publicRepository',
					'dependencies',
					'frontend',
					'native',
				].includes(key),
		)
	)
		throw new Error('Invalid public release input.')
	publicIdentity(input.version, { publicRepository: input.publicRepository })
	if (input.dependencies.length !== 1)
		throw new Error('Release input requires exactly one npm dependency.')
	for (const entry of input.dependencies)
		if (
			entry.name !== '@bluink/fonts' ||
			!versionPattern.test(entry.version) ||
			!/^sha512-[A-Za-z0-9+/]+={0,2}$/.test(entry.integrity) ||
			!/^[a-f0-9]{64}$/.test(entry.sha256) ||
			Object.keys(entry).some(
				(key) => !['name', 'version', 'integrity', 'sha256'].includes(key),
			)
		)
			throw new Error('Invalid fixed dependency identity.')
	validateFrontend(input.frontend)
	if (input.native !== undefined) validateNativeSelection(input.native)
	return input
}

export function validateContext(
	input,
	{
		commit,
		repository,
		ref,
		event,
		publicRepository,
		publishing = false,
		enabled,
		oidc,
	} = {},
) {
	validateInput(input)
	const identity = publicIdentity(
		input.version,
		{ publicRepository: input.publicRepository, publicCommit: commit },
		true,
	)
	if (
		repository &&
		identity.publicRepository !== `https://github.com/${repository}`
	)
		throw new Error(
			'Workflow repository does not match the frozen public identity.',
		)
	if (ref?.startsWith('refs/tags/') && ref !== `refs/tags/v${input.version}`)
		throw new Error('Release tag does not match the frozen version.')
	if (
		publishing &&
		(!repository ||
			event !== 'push' ||
			ref !== `refs/tags/v${input.version}` ||
			publicRepository !== 'true' ||
			enabled !== 'true' ||
			!oidc)
	)
		throw new Error(
			'Publication requires an enabled public tag workflow with OIDC.',
		)
	return identity
}

async function context(publishing = false) {
	const input = validateInput(await json(path.join(root, 'release-input.json')))
	const commit = (
		await run('git', ['rev-parse', 'HEAD'], { capture: true })
	).trim()
	if (process.env.GITHUB_SHA && process.env.GITHUB_SHA !== commit)
		throw new Error('Workflow checkout commit mismatch.')
	const identity = validateContext(input, {
		commit,
		repository: process.env.GITHUB_REPOSITORY,
		ref: process.env.GITHUB_REF,
		event: process.env.GITHUB_EVENT_NAME,
		publicRepository: process.env.BLUINK_PUBLIC_REPOSITORY,
		enabled: process.env.BLUINK_PUBLISH_ENABLED,
		oidc:
			process.env.GITHUB_ACTIONS === 'true' &&
			Boolean(
				process.env.ACTIONS_ID_TOKEN_REQUEST_URL &&
					process.env.ACTIONS_ID_TOKEN_REQUEST_TOKEN,
			),
		publishing,
	})
	const manifest = await json(path.join(root, 'backend/package.json'))
	if (
		manifest.version !== input.version ||
		Object.keys(manifest.dependencies).length !== 1 ||
		input.dependencies.some(
			(entry) => manifest.dependencies[entry.name] !== entry.version,
		)
	)
		throw new Error('Backend manifest differs from frozen release version.')
	if (
		(
			await run('git', ['status', '--porcelain', '--untracked-files=no'], {
				capture: true,
			})
		).trim()
	)
		throw new Error('Public source has uncommitted changes.')
	return { input, identity }
}

function frozenMatch(actual, expected) {
	if (
		!expected ||
		actual.name !== expected.name ||
		actual.version !== expected.version ||
		actual.sha256 !== expected.sha256 ||
		actual.integrity !== expected.integrity
	)
		throw new Error(`Artifact differs from frozen input: ${actual.name}`)
}

async function dependencies(input) {
	const destination = path.join(root, '.resource-packages')
	const records = await downloadResources(input.dependencies, destination)
	records.push(await downloadFrontend(input.frontend, destination))
	await writeJson(
		path.join(destination, 'dependencies.json'),
		records.map(resourceSummary),
	)
	return records
}

/** Verify the main package against the frozen inputs and the packed platform
 * packages it pins. `platforms` are archive records for every release target. */
export async function verifyCliArchive(
	archive,
	identity,
	records,
	platforms,
	nativeSelection,
) {
	const native = []
	for (const target of targets) {
		const record = platforms.find((entry) => entry.name === target.addonPackage)
		if (!record)
			throw new Error(`Missing platform package: ${target.addonPackage}`)
		if (nativeSelection)
			frozenMatch(
				record,
				nativeSelection.packages.find(({ name }) => name === record.name),
			)
		const { manifest, addon } = await verifyPlatformArchive(record, target, {
			...identity,
			version:
				nativeSelection?.version ?? nativePackageVersion(identity.version),
			publicCommit: nativeSelection?.publicCommit ?? identity.publicCommit,
		})
		native.push({ record: { ...record, manifest }, addon })
	}
	if (platforms.length !== targets.length)
		throw new Error('Unexpected platform package in the release.')
	const inspected = await inspectArchive(archive, {
		allowed: cliFileAllowed,
		required: [
			packageLauncher,
			'LICENSE',
			'README.md',
			'release.json',
			'npm-shrinkwrap.json',
			'THIRD_PARTY_NOTICES.txt',
		],
		onInspect: async (directory, { manifest, metadata }) => {
			const fields = new Set([
				'format',
				'formatVersion',
				'channel',
				'publicCommit',
				'publicRepository',
				'toolchain',
				'name',
				'version',
				'distribution',
				'contractHash',
				'dependencies',
				'frontend',
				'native',
				'nativeOrigin',
			])
			if (
				metadata?.format !== 'bluink.release' ||
				metadata.formatVersion !== 1 ||
				Object.keys(metadata).some((key) => !fields.has(key))
			)
				throw new Error('Unexpected CLI release metadata.')
			if (
				manifest.name !== packageName ||
				manifest.version !== identity.version ||
				metadata.name !== manifest.name ||
				metadata.version !== manifest.version ||
				manifest.repository?.url !== `${identity.publicRepository}.git` ||
				metadata?.distribution !== 'all-platforms' ||
				metadata.publicCommit !== identity.publicCommit ||
				metadata.publicRepository !== identity.publicRepository ||
				metadata.contractHash !== (await contractHash()) ||
				JSON.stringify(manifest.bin) !==
					JSON.stringify({ bluink: packageLauncher }) ||
				manifest.scripts !== undefined
			)
				throw new Error('CLI archive has the wrong public release identity.')
			const origin = nativeSelection && {
				releaseVersion: nativeSelection.releaseVersion,
				publicCommit: nativeSelection.publicCommit,
			}
			if (JSON.stringify(metadata.nativeOrigin) !== JSON.stringify(origin))
				throw new Error('CLI native origin differs from the frozen release.')
			const resources = records
			const web = records.find((entry) => entry.name === '@bluink/web')
			if (
				JSON.stringify(Object.keys(metadata.frontend ?? {}).sort()) !==
				JSON.stringify(['integrity', 'name', 'sha256', 'version'])
			)
				throw new Error('Unexpected bundled frontend identity metadata.')
			frozenMatch(metadata.frontend, web)
			if (
				web.metadata?.contractHash !== metadata.contractHash ||
				web.metadata?.fontsVersion !==
					records.find(({ name }) => name === '@bluink/fonts')?.version
			)
				throw new Error(
					'Web contract or Fonts version differs from the release.',
				)
			const expectedDependencies = resources
				.map(({ name, version, integrity }) => ({ name, version, integrity }))
				.sort((left, right) => left.name.localeCompare(right.name))
			if (
				JSON.stringify(
					metadata.dependencies?.toSorted((left, right) =>
						left.name.localeCompare(right.name),
					),
				) !== JSON.stringify(expectedDependencies) ||
				Object.keys(manifest.dependencies ?? {}).length !== 2 ||
				resources.some(
					(entry) => manifest.dependencies[entry.name] !== entry.version,
				)
			)
				throw new Error('CLI dependency identity differs from frozen inputs.')
			publicIdentity(
				manifest.version,
				Object.fromEntries(
					['channel', 'publicCommit', 'publicRepository', 'toolchain']
						.filter((key) => metadata[key] !== undefined)
						.map((key) => [key, metadata[key]]),
				),
				true,
			)
			if (
				JSON.stringify(manifest.optionalDependencies) !==
				JSON.stringify(
					Object.fromEntries(
						native.map(({ record }) => [record.name, record.version]),
					),
				)
			)
				throw new Error(
					'CLI platform packages differ from the release targets.',
				)
			if (
				JSON.stringify(metadata.native) !==
				JSON.stringify(
					native.map(({ record, addon }) => releaseNativeEntry(record, addon)),
				)
			)
				throw new Error('Native manifest differs from the platform packages.')
			if (
				!(await readFile(path.join(directory, packageLauncher))).equals(
					await readFile(path.join(root, 'scripts/package-launcher.mjs')),
				)
			)
				throw new Error('CLI launcher differs from this source.')
			const lock = await json(path.join(directory, 'npm-shrinkwrap.json'))
			if (
				JSON.stringify(lock) !==
				JSON.stringify(
					productionLock(manifest, [
						...records,
						...native.map(({ record }) => record),
					]),
				)
			)
				throw new Error('CLI production dependency lock changed.')
			return native.map(({ addon }) => addon)
		},
	})
	return inspected
}

async function assemble() {
	const { input, identity } = await context()
	if (!input.native) {
		const nativeIdentity = {
			...identity,
			version: nativePackageVersion(input.version),
		}
		const native = await verifyNativeArtifacts(
			nativeDirectory('release'),
			undefined,
			false,
			nativeIdentity,
		)
		validateNativeReports(nativeIdentity, native, await readNativeReports())
	}
	const records = await readPreparedResources(
		input,
		path.join(root, '.resource-packages'),
	)
	const metadata = { ...identity }
	delete metadata.version
	const { directory, platforms } = await buildPackage({
		version: input.version,
		metadata,
		dependencies: records,
		preparedDirectory: path.join(root, '.cli-build'),
		nativeSelection: input.native,
		nativeArchiveDirectory: path.join(root, '.resource-packages'),
	})
	const output = path.join(root, 'package-out')
	const cli = await packDirectory(directory, output, {
		allowed: cliFileAllowed,
	})
	await verifyCliArchive(
		cli.archive,
		{ ...identity, version: input.version },
		records,
		platforms,
		input.native,
	)
	for (const record of [...platforms, ...records])
		await copyFile(record.archive, path.join(output, record.filename))
	const summary = (record) => ({
		name: record.name,
		version: record.version,
		filename: record.filename,
		integrity: record.integrity,
		sha256: record.sha256,
	})
	await writeJson(path.join(output, 'manifest.json'), {
		format: 'bluink.public-build',
		formatVersion: 1,
		...identity,
		version: input.version,
		artifacts: [
			summary(cli),
			...platforms.map(summary),
			...records.map(summary),
		],
	})
}

async function nativeReport() {
	const { input, identity } = await context()
	const native = await verifyNativeArtifacts(
		nativeDirectory('release'),
		undefined,
		true,
		{
			version: nativePackageVersion(input.version),
			publicCommit: identity.publicCommit,
		},
	)
	await mkdir(path.join(root, 'verification'), { recursive: true })
	await writeJson(path.join(root, 'verification', `${native[0].target}.json`), {
		status: 'passed',
		publicCommit: identity.publicCommit,
		version: input.version,
		...native[0],
	})
}

async function readNativeReports() {
	return Object.fromEntries(
		await Promise.all(
			targets.map(async (target) => [
				target.rust,
				await json(path.join(root, 'verification', `${target.rust}.json`)),
			]),
		),
	)
}

/** Assembly must consume successful native checks from the same source and bytes. */
export function validateNativeReports(identity, artifacts, reports) {
	for (const target of targets) {
		const native = reports[target.rust]
		const artifact = artifacts.find((entry) => entry.target === target.rust)
		if (
			!artifact ||
			native?.status !== 'passed' ||
			native.version !== identity.version ||
			native.publicCommit !== identity.publicCommit ||
			native.target !== target.rust ||
			native.sha256 !== artifact.sha256 ||
			native.sourceDigest !== artifact.sourceDigest
		)
			throw new Error(`Missing matching native verification: ${target.rust}`)
	}
}

async function bundle() {
	const { input, identity } = await context()
	const manifest = await json(path.join(root, 'package-out/manifest.json'))
	if (
		manifest.format !== 'bluink.public-build' ||
		manifest.formatVersion !== 1 ||
		manifest.version !== input.version ||
		manifest.publicCommit !== identity.publicCommit ||
		manifest.publicRepository !== identity.publicRepository ||
		manifest.artifacts?.length !== 3 + targets.length ||
		Object.keys(manifest).some(
			(key) =>
				![
					'format',
					'formatVersion',
					'channel',
					'version',
					'publicCommit',
					'publicRepository',
					'artifacts',
				].includes(key),
		)
	)
		throw new Error('Invalid prepared public build manifest.')
	const records = []
	for (const entry of manifest.artifacts) {
		if (
			entry.filename !==
				`${entry.name.replace('@', '').replace('/', '-')}-${entry.version}.tgz` ||
			Object.keys(entry).some(
				(key) =>
					!['name', 'version', 'filename', 'integrity', 'sha256'].includes(key),
			)
		)
			throw new Error('Unsafe prepared archive filename or metadata.')
		const archive = path.join(root, 'package-out', entry.filename)
		if ((await fileHash(archive)) !== entry.sha256)
			throw new Error(`Prepared archive changed: ${entry.name}`)
		records.push({ ...entry, archive })
	}
	const cli = records.find((entry) => entry.name === packageName)
	const platformNames = targets.map((target) => target.addonPackage)
	const platforms = records.filter((entry) =>
		platformNames.includes(entry.name),
	)
	if (
		!cli ||
		platforms.length !== targets.length ||
		new Set(records.map((entry) => entry.name)).size !== 3 + targets.length
	)
		throw new Error('Incomplete prepared archive set.')
	const dependencies = []
	for (const record of records.filter(
		(entry) => entry !== cli && !platforms.includes(entry),
	)) {
		frozenMatch(
			record,
			record.name === '@bluink/web'
				? input.frontend
				: input.dependencies.find((entry) => entry.name === record.name),
		)
		const inspected = await verifyArchive(
			record,
			resourceAudit(record.name.split('/')[1]),
		)
		dependencies.push({ ...record, ...inspected })
	}
	await verifyArchive(cli, { allowed: cliFileAllowed })
	const inspected = await verifyCliArchive(
		cli.archive,
		{ ...identity, version: input.version },
		dependencies,
		platforms,
		input.native,
	)
	return {
		input,
		identity,
		manifest,
		cli,
		platforms,
		dependencies,
		nativeArtifacts: inspected.inspection,
	}
}

async function smoke() {
	const { input, identity, cli, platforms, dependencies } = await bundle()
	const host = platforms.find(
		(entry) => entry.name === nativeHostTarget().addonPackage,
	)
	const report = await smokePackage(cli.archive, input.version, [
		host.archive,
		...dependencies.map((entry) => entry.archive),
	])
	await mkdir(path.join(root, 'smoke-results'), { recursive: true })
	await writeJson(
		path.join(
			root,
			'smoke-results',
			`${process.platform}-${process.arch}.json`,
		),
		{
			...report,
			version: input.version,
			publicCommit: identity.publicCommit,
			archiveSha256: cli.sha256,
		},
	)
}

export function validateReports(
	{ input, manifest, cli, nativeArtifacts },
	reports,
) {
	if (!input.native)
		validateNativeReports(
			{
				version: nativePackageVersion(input.version),
				publicCommit: manifest.publicCommit,
			},
			nativeArtifacts,
			Object.fromEntries(
				Object.entries(reports).map(([target, report]) => [
					target,
					report.native,
				]),
			),
		)
	return targets.map((target) => {
		const { smoke: report } = reports[target.rust] ?? {}
		const artifact = nativeArtifacts.find(
			(entry) => entry.target === target.rust,
		)
		if (
			!artifact ||
			report?.status !== 'passed' ||
			report.platform !== target.platform ||
			report.arch !== target.arch ||
			report.version !== input.version ||
			report.publicCommit !== manifest.publicCommit ||
			report.archiveSha256 !== cli.sha256
		)
			throw new Error(`Missing matching verification: ${target.rust}`)
		return {
			target: target.rust,
			platform: target.platform,
			arch: target.arch,
			status: 'passed',
			nativeSha256: artifact.sha256,
			archiveSha256: cli.sha256,
		}
	})
}

async function verifiedBundle() {
	const prepared = await bundle()
	const reports = Object.fromEntries(
		await Promise.all(
			targets.map(async (target) => [
				target.rust,
				{
					smoke: await json(
						path.join(
							root,
							'smoke-results',
							`${target.platform}-${target.arch}.json`,
						),
					),
					native: prepared.input.native
						? undefined
						: await json(
								path.join(root, 'verification', `${target.rust}.json`),
							),
				},
			]),
		),
	)
	const checks = validateReports(prepared, reports)
	const evidence = {
		format: 'bluink.public-verification',
		formatVersion: 1,
		version: prepared.input.version,
		publicCommit: prepared.manifest.publicCommit,
		manifestSha256: await fileHash(
			path.join(root, 'package-out/manifest.json'),
		),
		checks,
	}
	await writeJson(path.join(root, 'package-out/verification.json'), evidence)
	return prepared
}

async function publish() {
	await context(true)
	const {
		input,
		identity: contextIdentity,
		cli,
		platforms,
		dependencies,
	} = await verifiedBundle()
	const identity = {
		...contextIdentity,
		version: cli.version,
		nativeOrigin: input.native,
	}
	const tagFor = (record) =>
		record.name === packageName
			? identity.channel === 'stable'
				? 'next'
				: identity.channel
			: 'latest'
	await verifyRegistryProvenance(dependencies, identity)
	// Platform packages first: the main package's shrinkwrap pins them.
	const publishing = input.native ? [cli] : [...platforms, cli]
	await publishRecords(publishing, {
		tagFor,
		verifyExisting: (records) => verifyRegistryProvenance(records, identity),
	})
	await writeJson(
		path.join(root, 'package-out/registry-verification.json'),
		await verifyRegistryProvenance(
			[...dependencies, ...platforms, cli],
			identity,
		),
	)
	for (const record of publishing)
		console.log(
			'Verified npm distribution tag:',
			await verifyRegistryTag(record.name, record.version, {
				tag: tagFor(record),
			}),
		)
}

if (
	process.argv[1] &&
	path.resolve(process.argv[1]) === fileURLToPath(import.meta.url)
) {
	const { values, positionals } = parseArgs({
		allowPositionals: true,
		options: { target: { type: 'string' } },
	})
	if (positionals.length !== 1)
		throw new Error(
			'Choose context, resources, resources-install, native-report, cli-build, assemble, smoke, verify or publish.',
		)
	if (positionals[0] === 'context') {
		await context()
		if (
			values.target &&
			!targets.some(
				(target) =>
					target.rust === values.target &&
					target.platform === process.platform &&
					target.arch === process.arch,
			)
		)
			throw new Error(
				'Runner platform/architecture does not match the requested target.',
			)
	} else if (positionals[0] === 'resources') {
		const { input, identity } = await context()
		await verifyRegistryProvenance(await dependencies(input), identity)
		if (input.native) {
			const directory = path.join(root, '.resource-packages')
			await downloadNativeComponents(input.native, directory)
			const records = await readNativeComponents(
				input.native,
				directory,
				identity.publicRepository,
			)
			await verifyRegistryProvenance(records, {
				...identity,
				nativeOrigin: input.native,
			})
		}
	} else if (positionals[0] === 'resources-install') {
		const { input } = await context()
		await readPreparedResources(
			input,
			path.join(root, '.resource-packages'),
			root,
		)
		await stageInstalledResources()
	} else if (positionals[0] === 'native-report') await nativeReport()
	else if (positionals[0] === 'cli-build') {
		const { input, identity } = await context()
		await prepareCliBuild(path.join(root, '.cli-build'), {
			...identity,
			version: input.version,
		})
	} else if (positionals[0] === 'assemble') await assemble()
	else if (positionals[0] === 'smoke') await smoke()
	else if (positionals[0] === 'verify') await verifiedBundle()
	else if (positionals[0] === 'publish') await publish()
	else throw new Error('Unknown public release operation.')
}
