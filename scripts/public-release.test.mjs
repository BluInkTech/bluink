// SPDX-License-Identifier: MPL-2.0
import assert from 'node:assert/strict'
import test from 'node:test'
import {
	appendFile,
	chmod,
	copyFile,
	mkdir,
	mkdtemp,
	readFile,
	rm,
	utimes,
	writeFile,
} from 'node:fs/promises'
import os from 'node:os'
import path from 'node:path'
import { contractHash } from './cli-build.mjs'
import { frontendInput, stageFrontend } from './frontend-asset.mjs'
import { packDirectory, productionLock } from './package-artifacts.mjs'
import { cliFileAllowed } from './package-audit.mjs'
import { readPreparedResources, resourceSummary } from './resource-packages.mjs'
import {
	validateContext,
	validateInput,
	validateNativeReports,
	validateReports,
	verifyCliArchive,
} from './public-release.mjs'
import {
	nativeSourceDigest,
	targets,
	verifyNativeArtifacts,
	writeJson,
} from './release-utils.mjs'

test('shared resources relocate across runners, verify all archives before staging and reject tampering', async () => {
	const temporary = await mkdtemp(
		path.join(os.tmpdir(), 'bluink-shared-resources-'),
	)
	try {
		const entries = []
		const shared = path.join(temporary, 'downloaded')
		await mkdir(shared)
		for (const kind of ['fonts', 'typst', 'web']) {
			const source = path.join(temporary, kind)
			const file =
				kind === 'web'
					? 'assets/index.html'
					: kind === 'fonts'
						? 'assets/web/inter.css'
						: 'assets/windows-x64/LICENSE'
			await mkdir(path.dirname(path.join(source, file)), { recursive: true })
			await writeJson(path.join(source, 'package.json'), {
				name: `@bluink/${kind}`,
				version: '26.10.7-alpha',
				files: [
					'assets',
					...(kind === 'web'
						? ['release.json', 'LICENSE', 'THIRD_PARTY_NOTICES.txt']
						: []),
				],
			})
			await writeFile(path.join(source, file), 'Shared resource fixture.\n')
			if (kind === 'web') {
				await writeJson(path.join(source, 'release.json'), {
					version: '26.10.7-alpha',
				})
				for (const name of ['LICENSE', 'THIRD_PARTY_NOTICES.txt'])
					await writeFile(
						path.join(source, name),
						'Frontend fixture licence.\n',
					)
			}
			const record = await packDirectory(
				source,
				path.join(temporary, 'prepared'),
				{ allowed: () => true },
			)
			await copyFile(record.archive, path.join(shared, record.filename))
			entries.push(resourceSummary(record))
		}
		const input = {
			dependencies: entries
				.filter((entry) => entry.name !== '@bluink/web')
				.map(({ filename: _, ...identity }) => identity),
			frontend: entries.find((entry) => entry.name === '@bluink/web'),
		}
		const metadata = path.join(shared, 'dependencies.json')
		await writeJson(metadata, entries)
		const installed = path.join(temporary, 'workspace')
		const records = await readPreparedResources(input, shared, installed)
		assert.equal(records.length, 3)
		assert.ok(
			records.every((record) => path.dirname(record.archive) === shared),
		)
		assert.equal(
			await readFile(
				path.join(installed, 'fonts/assets/web/inter.css'),
				'utf8',
			),
			'Shared resource fixture.\n',
		)
		assert.equal(
			await readFile(
				path.join(installed, 'typst/assets/windows-x64/LICENSE'),
				'utf8',
			),
			'Shared resource fixture.\n',
		)
		for (const invalid of [
			entries.slice(1),
			[entries[0], entries[0], entries[2]],
			entries.map((entry) => ({
				...entry,
				archive: 'C:\\runner\\absolute.tgz',
			})),
			entries.map((entry) => ({ ...entry, filename: '../outside.tgz' })),
			entries.map((entry) => ({ ...entry, sha256: 'f'.repeat(64) })),
		]) {
			await writeJson(metadata, invalid)
			await assert.rejects(
				readPreparedResources(input, shared),
				/shared resource|Shared resource/,
			)
		}
		await writeJson(metadata, entries)
		await writeFile(
			path.join(installed, 'fonts/assets/web/inter.css'),
			'Preserve until all archives verify.',
		)
		await appendFile(path.join(shared, entries[2].filename), 'tampered')
		await assert.rejects(
			readPreparedResources(input, shared, installed),
			/integrity mismatch/,
		)
		assert.equal(
			await readFile(
				path.join(installed, 'fonts/assets/web/inter.css'),
				'utf8',
			),
			'Preserve until all archives verify.',
		)
	} finally {
		await rm(temporary, { recursive: true, force: true })
	}
})

test('public release context binds version, repository, tag and protected OIDC publication', () => {
	const input = {
		format: 'bluink.release-input',
		formatVersion: 1,
		version: '26.10.7-alpha',
		publicRepository: 'https://github.com/example/cli',
		frontend: frontendInput(
			{
				version: '26.10.7-alpha',
				integrity: 'sha512-Ynl0ZXM=',
				sha256: 'c'.repeat(64),
			},
			'https://github.com/example/cli',
		),
		dependencies: ['fonts', 'typst'].map((kind) => ({
			name: `@bluink/${kind}`,
			version: '26.10.7-alpha',
			integrity: 'sha512-Ynl0ZXM=',
			sha256: 'c'.repeat(64),
		})),
	}
	const context = {
		commit: 'a'.repeat(40),
		repository: 'example/cli',
		ref: 'refs/tags/v26.10.7-alpha',
		event: 'push',
		publicRepository: 'true',
		enabled: 'true',
		oidc: true,
		publishing: true,
	}
	assert.equal(validateContext(input, context).publicCommit, context.commit)
	for (const override of [
		{ repository: 'private/repo' },
		{ ref: 'refs/tags/v26.10.8-alpha' },
		{ publicRepository: 'false' },
		{ enabled: 'false' },
		{ event: 'workflow_dispatch' },
		{ oidc: false },
	])
		assert.throws(
			() => validateContext(input, { ...context, ...override }),
			/repository|tag|Publication/,
		)
	assert.throws(
		() =>
			validateInput({ ...input, dependencies: input.dependencies.slice(1) }),
		/two npm dependencies/,
	)
	assert.throws(
		() =>
			validateInput({
				...input,
				dependencies: input.dependencies.map((entry) => ({
					...entry,
					sha256: 'invalid',
				})),
			}),
		/dependency identity/,
	)
	assert.throws(
		() =>
			validateInput({
				...input,
				frontend: { ...input.frontend, version: '26.10.8-alpha' },
			}),
		/frontend asset identity or version/,
	)
	assert.throws(
		() => validateInput({ ...input, privateRevision: 'b'.repeat(40) }),
		/Invalid public/,
	)
	assert.throws(
		() =>
			validateInput({
				...input,
				dependencies: input.dependencies.map((entry) => ({
					...entry,
					archive: '/temporary/path',
				})),
			}),
		/dependency identity/,
	)
	assert.throws(
		() => validateInput({ ...input, version: '0.0.0-dev' }),
		/Invalid public/,
	)
})

test('official archive verification inspects embedded native identities and dependency metadata', async () => {
	const directory = await mkdtemp(
		path.join(os.tmpdir(), 'bluink-public-archive-'),
	)
	try {
		const source = path.join(directory, 'input')
		await mkdir(path.join(source, 'runtime'), { recursive: true })
		await mkdir(path.join(source, 'dist'))
		const identity = {
			version: '26.10.7-alpha',
			publicCommit: 'a'.repeat(40),
			publicRepository: 'https://github.com/example/cli',
		}
		const dependencies = ['fonts', 'typst'].map((kind) => ({
			name: `@bluink/${kind}`,
			version: identity.version,
			integrity: 'sha512-Ynl0ZXM=',
			manifest: {
				license: kind === 'web' ? 'SEE LICENSE IN LICENSE' : 'MPL-2.0',
			},
		}))
		const webSource = path.join(directory, 'frontend')
		await mkdir(path.join(webSource, 'assets'), { recursive: true })
		await writeJson(path.join(webSource, 'package.json'), {
			name: '@bluink/web',
			version: identity.version,
			private: true,
			files: ['assets', 'release.json', 'LICENSE', 'THIRD_PARTY_NOTICES.txt'],
		})
		await writeJson(path.join(webSource, 'release.json'), {
			version: identity.version,
		})
		for (const file of [
			'assets/index.html',
			'LICENSE',
			'THIRD_PARTY_NOTICES.txt',
		])
			await writeFile(path.join(webSource, file), 'Frontend fixture.\n')
		const web = await packDirectory(
			webSource,
			path.join(directory, 'frontend-packed'),
			{ allowed: () => true },
		)
		await stageFrontend(web, path.join(source, 'web'))
		const records = [...dependencies, web]
		const manifest = {
			name: '@bluink/cli',
			version: identity.version,
			license: 'MPL-2.0',
			repository: { url: `${identity.publicRepository}.git` },
			dependencies: Object.fromEntries(
				dependencies.map(({ name, version }) => [name, version]),
			),
			files: [
				'bluink',
				'dist',
				'runtime',
				'web',
				'LICENSE',
				'README.md',
				'release.json',
				'npm-shrinkwrap.json',
				'THIRD_PARTY_NOTICES.txt',
			],
		}
		const digest = await nativeSourceDigest()
		for (const target of targets)
			await writeFile(
				path.join(source, 'runtime', target.artifact),
				`BLUINK_NATIVE_BUILD_INFO_V1:${JSON.stringify({ sourceDigest: digest, target: target.rust, mode: 'release', version: identity.version, publicCommit: identity.publicCommit })}:END_BLUINK_NATIVE_BUILD_INFO`,
			)
		for (const file of [
			'bluink',
			'dist/index.js',
			'runtime/index.cjs',
			'LICENSE',
			'README.md',
			'THIRD_PARTY_NOTICES.txt',
		])
			await writeFile(
				path.join(source, file),
				'Synthetic package verification fixture.\n',
			)
		const metadata = {
			format: 'bluink.release',
			formatVersion: 1,
			channel: 'alpha',
			...identity,
			name: manifest.name,
			distribution: 'all-platforms',
			contractHash: await contractHash(),
			frontend: {
				name: web.name,
				version: web.version,
				integrity: web.integrity,
				sha256: web.sha256,
			},
			dependencies: dependencies.map(({ name, version, integrity }) => ({
				name,
				version,
				integrity,
			})),
			native: await verifyNativeArtifacts(
				path.join(source, 'runtime'),
				undefined,
				false,
				identity,
			),
		}
		await writeJson(path.join(source, 'package.json'), manifest)
		await writeJson(
			path.join(source, 'npm-shrinkwrap.json'),
			productionLock(manifest, dependencies),
		)
		const packed = async () => {
			await writeJson(path.join(source, 'release.json'), metadata)
			return packDirectory(source, path.join(directory, 'output'), {
				allowed: cliFileAllowed,
			})
		}
		const first = await packed()
		await chmod(path.join(source, 'dist/index.js'), 0o755)
		await utimes(path.join(source, 'dist/index.js'), new Date(), new Date())
		assert.equal((await packed()).sha256, first.sha256)
		assert.equal(
			(await verifyCliArchive(first.archive, identity, records)).inspection
				.length,
			3,
		)
		metadata.dependencies[0].integrity = 'sha512-Y2hhbmdlZA=='
		await assert.rejects(
			verifyCliArchive((await packed()).archive, identity, records),
			/dependency identity/,
		)
		metadata.dependencies[0].integrity = dependencies[0].integrity
		metadata.version = '26.10.8-alpha'
		await assert.rejects(
			verifyCliArchive((await packed()).archive, identity, records),
			/public release identity/,
		)
		metadata.version = identity.version
		metadata.native[0].sha256 = 'f'.repeat(64)
		await assert.rejects(
			verifyCliArchive((await packed()).archive, identity, records),
			/Native manifest/,
		)
	} finally {
		await rm(directory, { recursive: true, force: true })
	}
})

test('publication requires all three reports bound to this exact archive and native source', () => {
	const version = '26.10.7-alpha'
	const publicCommit = 'a'.repeat(40)
	const archiveSha256 = 'b'.repeat(64)
	const prepared = {
		input: { version },
		manifest: { publicCommit },
		cli: { sha256: archiveSha256 },
		nativeArtifacts: targets.map((target) => ({
			target: target.rust,
			sha256: 'c'.repeat(64),
			sourceDigest: 'd'.repeat(64),
		})),
	}
	const reports = Object.fromEntries(
		targets.map((target) => [
			target.rust,
			{
				smoke: {
					status: 'passed',
					platform: target.platform,
					arch: target.arch,
					version,
					publicCommit,
					archiveSha256,
				},
				native: {
					status: 'passed',
					version,
					publicCommit,
					target: target.rust,
					sha256: 'c'.repeat(64),
					sourceDigest: 'd'.repeat(64),
				},
			},
		]),
	)
	assert.equal(validateReports(prepared, reports).length, 3)
	for (const change of [
		(report) => {
			report.smoke.archiveSha256 = 'e'.repeat(64)
		},
		(report) => {
			report.native.sourceDigest = 'e'.repeat(64)
		},
		(report) => {
			report.smoke.arch = 'wrong'
		},
		(report) => {
			report.native.publicCommit = 'f'.repeat(40)
		},
		(report) => {
			report.smoke.status = 'failed'
		},
	]) {
		const modified = structuredClone(reports)
		change(modified[targets[0].rust])
		assert.throws(() => validateReports(prepared, modified), /Missing matching/)
	}
	assert.throws(() => validateReports(prepared, {}), /Missing matching/)
})

test('assembly refuses missing, stale or altered native verification before packaging', () => {
	const identity = { version: '26.10.7-alpha', publicCommit: 'a'.repeat(40) }
	const artifacts = targets.map((target) => ({
		target: target.rust,
		sha256: 'b'.repeat(64),
		sourceDigest: 'c'.repeat(64),
	}))
	const reports = Object.fromEntries(
		artifacts.map((artifact) => [
			artifact.target,
			{ ...identity, ...artifact, status: 'passed' },
		]),
	)
	validateNativeReports(identity, artifacts, reports)
	for (const change of [
		(value) => {
			delete value[targets[0].rust]
		},
		(value) => {
			value[targets[0].rust].version = '26.10.8-alpha'
		},
		(value) => {
			value[targets[0].rust].publicCommit = 'd'.repeat(40)
		},
		(value) => {
			value[targets[0].rust].sha256 = 'e'.repeat(64)
		},
		(value) => {
			value[targets[0].rust].sourceDigest = 'f'.repeat(64)
		},
		(value) => {
			value[targets[0].rust].status = 'failed'
		},
	]) {
		const modified = structuredClone(reports)
		change(modified)
		assert.throws(
			() => validateNativeReports(identity, artifacts, modified),
			/Missing matching native/,
		)
	}
})
