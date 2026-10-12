// SPDX-License-Identifier: MPL-2.0
// Copyright (c) 2023–2026 BluInk

import { cp, mkdir, mkdtemp, rm } from 'node:fs/promises'
import os from 'node:os'
import path from 'node:path'
import { fileURLToPath } from 'node:url'
import { parseArgs } from 'node:util'
import { verifyArchive } from './package-artifacts.mjs'
import { publicIdentity, versionPattern } from './release-identity.mjs'
import {
	publishRecords,
	verifyRegistryProvenance,
} from './registry-publication.mjs'
import {
	packResource,
	resourceAudit,
	resourceSummary,
} from './resource-packages.mjs'
import { json, npm, root, run, targets, writeJson } from './release-utils.mjs'

export function validateResourceInput(input) {
	if (
		input?.format !== 'bluink.resource-input' ||
		input.formatVersion !== 1 ||
		!['fonts', 'typst'].includes(input.kind) ||
		!versionPattern.test(input.version) ||
		!input.publicRepository ||
		Object.keys(input).some(
			(key) =>
				![
					'format',
					'formatVersion',
					'kind',
					'version',
					'publicRepository',
					'artifact',
				].includes(key),
		)
	)
		throw new Error('Invalid public resource input.')
	publicIdentity(input.version, { publicRepository: input.publicRepository })
	const artifact = input.artifact
	if (
		artifact?.name !== `@bluink/${input.kind}` ||
		artifact.version !== input.version ||
		artifact.filename !== `bluink-${input.kind}-${input.version}.tgz` ||
		!/^sha512-[A-Za-z0-9+/]+={0,2}$/.test(artifact.integrity) ||
		!/^[a-f0-9]{64}$/.test(artifact.sha256) ||
		Object.keys(artifact).some(
			(key) =>
				!['name', 'version', 'filename', 'integrity', 'sha256'].includes(key),
		)
	)
		throw new Error('Invalid frozen resource archive identity.')
	return input
}

export function validateResourceContext(
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
	validateResourceInput(input)
	const identity = publicIdentity(
		input.version,
		{
			publicRepository: input.publicRepository,
			publicCommit: commit,
		},
		true,
	)
	const tag = `refs/tags/${input.kind}/v${input.version}`
	if (
		repository &&
		input.publicRepository !== `https://github.com/${repository}`
	)
		throw new Error(
			'Workflow repository differs from the frozen resource identity.',
		)
	if (ref?.startsWith('refs/tags/') && ref !== tag)
		throw new Error('Resource tag differs from the frozen version.')
	if (
		publishing &&
		(!repository ||
			event !== 'push' ||
			ref !== tag ||
			publicRepository !== 'true' ||
			enabled !== 'true' ||
			!oidc)
	)
		throw new Error(
			'Resource publication requires an enabled public tag workflow with OIDC.',
		)
	return { ...identity, packageName: input.artifact.name }
}

export function validateResourceReports(input, identity, reports) {
	return targets.map((target) => {
		const report = reports[`${target.platform}-${target.arch}`]
		if (
			report?.status !== 'passed' ||
			report.kind !== input.kind ||
			report.version !== input.version ||
			report.publicCommit !== identity.publicCommit ||
			report.platform !== target.platform ||
			report.arch !== target.arch ||
			report.archiveSha256 !== input.artifact.sha256
		)
			throw new Error(
				`Missing matching ${input.kind} smoke report: ${target.platform}/${target.arch}`,
			)
		return report
	})
}

function sameArtifact(actual, expected) {
	return ['name', 'version', 'filename', 'integrity', 'sha256'].every(
		(key) => actual?.[key] === expected[key],
	)
}

async function context(kind, publishing = false) {
	if (!['fonts', 'typst'].includes(kind))
		throw new Error('Choose fonts or typst.')
	const input = validateResourceInput(
		await json(path.join(root, '.resource-inputs', `${kind}.json`)),
	)
	if (input.kind !== kind)
		throw new Error('Resource input belongs to a different package.')
	const commit = (
		await run('git', ['rev-parse', 'HEAD'], { capture: true })
	).trim()
	if (process.env.GITHUB_SHA && process.env.GITHUB_SHA !== commit)
		throw new Error('Resource workflow checkout commit mismatch.')
	const identity = validateResourceContext(input, {
		commit,
		repository: process.env.GITHUB_REPOSITORY,
		ref: process.env.GITHUB_REF,
		event: process.env.GITHUB_EVENT_NAME,
		publicRepository: process.env.BLUINK_PUBLIC_REPOSITORY,
		enabled: process.env.BLUINK_RESOURCE_PUBLISH_ENABLED,
		publishing,
		oidc:
			process.env.GITHUB_ACTIONS === 'true' &&
			Boolean(
				process.env.ACTIONS_ID_TOKEN_REQUEST_URL &&
					process.env.ACTIONS_ID_TOKEN_REQUEST_TOKEN,
			),
	})
	const manifest = await json(path.join(root, kind, 'package.json'))
	if (
		manifest.name !== input.artifact.name ||
		manifest.version !== input.version
	)
		throw new Error('Resource source manifest differs from the frozen version.')
	if (
		(
			await run('git', ['status', '--porcelain', '--untracked-files=no'], {
				capture: true,
			})
		).trim()
	)
		throw new Error('Public resource source has uncommitted changes.')
	return { input, identity, manifest }
}

async function candidate(input, identity) {
	const directory = path.join(root, '.resource-out', input.kind)
	const manifest = await json(path.join(directory, 'manifest.json'))
	if (
		manifest.format !== 'bluink.resource-artifact' ||
		manifest.formatVersion !== 1 ||
		manifest.publicCommit !== identity.publicCommit ||
		manifest.publicRepository !== identity.publicRepository ||
		!sameArtifact(manifest.artifact, input.artifact)
	)
		throw new Error(
			'Prepared resource artifact differs from the frozen source identity.',
		)
	return verifyArchive(
		{
			...input.artifact,
			archive: path.join(directory, input.artifact.filename),
		},
		await resourceAudit(input.kind),
	)
}

export async function prepareResource(input, identity) {
	const directory = path.join(root, '.resource-out', input.kind)
	await npm(['run', 'build', '--workspaces=false'], {
		cwd: path.join(root, input.kind),
	})
	const record = await packResource(input.kind, directory, {
		repository: input.publicRepository,
	})
	if (!sameArtifact(resourceSummary(record), input.artifact))
		throw new Error(
			'Rebuilt resource archive differs from the frozen candidate.',
		)
	await writeJson(path.join(directory, 'manifest.json'), {
		format: 'bluink.resource-artifact',
		formatVersion: 1,
		publicCommit: identity.publicCommit,
		publicRepository: identity.publicRepository,
		artifact: input.artifact,
	})
	return record
}

/** Exercise installed package code and assets, never source-tree replacements. */
export async function smokeResource(input, identity, sourceManifest) {
	const target = targets.find(
		(target) =>
			target.platform === process.platform && target.arch === process.arch,
	)
	if (!target) throw new Error('Unsupported resource smoke-test platform.')
	const record = await candidate(input, identity)
	const temporary = await mkdtemp(
		path.join(os.tmpdir(), 'bluink-resource-smoke-'),
	)
	try {
		const dependencies = { [record.name]: `file:${record.archive}` }
		if (input.kind === 'fonts') {
			const version = sourceManifest.devDependencies?.['@bluink/typst']
			if (!versionPattern.test(version))
				throw new Error(
					'Fonts smoke tests require an exact Typst test dependency.',
				)
			dependencies['@bluink/typst'] = version
		}
		await writeJson(path.join(temporary, 'package.json'), {
			private: true,
			type: 'module',
			dependencies,
		})
		await npm(['install', '--ignore-scripts', '--no-audit', '--no-fund'], {
			cwd: temporary,
		})
		const installed = path.join(
			temporary,
			'node_modules',
			'@bluink',
			input.kind,
		)
		const manifest = await json(path.join(installed, 'package.json'))
		if (manifest.name !== record.name || manifest.version !== record.version)
			throw new Error('Installed resource identity mismatch.')
		const tests = path.join(installed, 'tests')
		await mkdir(tests)
		await cp(
			path.join(root, input.kind, 'tests/resources.test.mjs'),
			path.join(tests, 'resources.test.mjs'),
		)
		await run(
			process.execPath,
			['--test', path.join(tests, 'resources.test.mjs')],
			{ cwd: os.tmpdir() },
		)
		const result = {
			status: 'passed',
			kind: input.kind,
			version: input.version,
			publicCommit: identity.publicCommit,
			platform: process.platform,
			arch: process.arch,
			archiveSha256: record.sha256,
		}
		const directory = path.join(root, '.resource-smoke', input.kind)
		await mkdir(directory, { recursive: true })
		await writeJson(
			path.join(directory, `${process.platform}-${process.arch}.json`),
			result,
		)
		return result
	} finally {
		await rm(temporary, { recursive: true, force: true })
	}
}

async function verifiedReports(input, identity) {
	const reports = {}
	for (const target of targets) {
		const key = `${target.platform}-${target.arch}`
		reports[key] = await json(
			path.join(root, '.resource-smoke', input.kind, `${key}.json`),
		)
	}
	return validateResourceReports(input, identity, reports)
}

if (
	process.argv[1] &&
	path.resolve(process.argv[1]) === fileURLToPath(import.meta.url)
) {
	const { positionals, values } = parseArgs({
		allowPositionals: true,
		options: { package: { type: 'string' } },
	})
	if (
		positionals.length !== 1 ||
		!['context', 'prepare', 'smoke', 'verify', 'publish'].includes(
			positionals[0],
		)
	)
		throw new Error(
			'Choose context, prepare, smoke, verify or publish with --package fonts|typst.',
		)
	const action = positionals[0]
	const { input, identity, manifest } = await context(
		values.package,
		action === 'publish',
	)
	if (action === 'prepare') await prepareResource(input, identity)
	if (action === 'smoke') await smokeResource(input, identity, manifest)
	if (['verify', 'publish'].includes(action)) {
		const record = await candidate(input, identity)
		const reports = await verifiedReports(input, identity)
		await writeJson(
			path.join(root, '.resource-out', input.kind, 'verification.json'),
			{
				status: 'passed',
				publicCommit: identity.publicCommit,
				artifact: input.artifact,
				reports,
			},
		)
		if (action === 'publish') {
			await publishRecords([record], {
				verifyExisting: (records) =>
					verifyRegistryProvenance(records, identity),
			})
			await writeJson(
				path.join(
					root,
					'.resource-out',
					input.kind,
					'registry-verification.json',
				),
				{
					status: 'passed',
					packages: await verifyRegistryProvenance([record], identity),
				},
			)
		}
	}
}
