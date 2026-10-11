// SPDX-License-Identifier: MPL-2.0
// Copyright (c) 2023–2026 BluInk

import { chmod, cp, mkdir, readFile, rm } from 'node:fs/promises'
import path from 'node:path'
import { fileURLToPath } from 'node:url'
import { productionLock, verifyArchive } from './package-artifacts.mjs'
import { publicIdentity } from './release-identity.mjs'
import { resourceFileAllowed } from './resource-packages.mjs'
import {
	auditDirectory,
	cliFileAllowed,
	webFileAllowed,
} from './package-audit.mjs'
import {
	json,
	npm,
	root,
	sha256,
	verifyNativeArtifacts,
	writeJson,
} from './release-utils.mjs'
import { rustNotices } from './rust-notices.mjs'

export async function contractHash() {
	return sha256(
		Buffer.concat(
			await Promise.all(
				['types.ts', 'runtime.ts', 'wire.ts'].map(async (name) =>
					Buffer.concat([
						Buffer.from(`${name}\0`),
						await readFile(
							path.join(root, 'backend/contracts/generated', name),
						),
					]),
				),
			),
		),
	)
}

/** Public assembly consumes fixed archives; it never compiles frontend source. */
export async function buildPackage({
	version,
	metadata,
	nativeDirectory = path.join(root, 'backend/runtime'),
	dependencies,
	hostOnly = false,
} = {}) {
	if (!dependencies?.length)
		throw new Error(
			'CLI assembly requires verified Web, Fonts and Typst archives. Use npm run build in the private workspace or pass --dependencies in a public checkout.',
		)
	await npm(['run', 'contracts:check'])
	const source = await json(path.join(root, 'backend/package.json'))
	const productVersion = version ?? source.version
	const identity = publicIdentity(productVersion, metadata, !hostOnly)
	const native = await verifyNativeArtifacts(
		nativeDirectory,
		undefined,
		hostOnly,
		{ version: productVersion, publicCommit: identity.publicCommit },
	)
	if (
		dependencies.length !== 3 ||
		new Set(dependencies.map((entry) => entry.name)).size !== 3 ||
		dependencies.some(
			(entry) =>
				!['@bluink/web', '@bluink/fonts', '@bluink/typst'].includes(entry.name),
		)
	)
		throw new Error('Supply exactly one Web, Fonts and Typst archive.')
	const pins = await json(path.join(root, 'typst/sources.json'))
	const trustedUpstream = Object.fromEntries(
		Object.entries(pins.targets).map(([target, spec]) => [
			`assets/${target}/${spec.binary}`,
			spec.binarySha256,
		]),
	)
	dependencies = await Promise.all(
		dependencies.map((entry) =>
			verifyArchive(
				entry,
				entry.name === '@bluink/web'
					? { allowed: webFileAllowed, frontend: true }
					: {
							allowed: (file) =>
								resourceFileAllowed(entry.name.split('/')[1], file),
							...(entry.name === '@bluink/typst' && { trustedUpstream }),
						},
			),
		),
	)
	const web = dependencies.find((entry) => entry.name === '@bluink/web')
	if (!web || web.version !== productVersion)
		throw new Error('CLI and Web package versions must match exactly.')
	const inspected = web
	const fonts = dependencies.find((entry) => entry.name === '@bluink/fonts')
	if (
		inspected.metadata?.contractHash !== (await contractHash()) ||
		inspected.metadata?.fontsVersion !== fonts?.version ||
		inspected.metadata?.fontCatalogueHash !==
			sha256(await readFile(path.join(root, 'fonts/catalogue.json')))
	)
		throw new Error(
			'Web contract or font catalogue identity does not match this backend.',
		)
	await rm(path.join(root, 'backend/dist'), { recursive: true, force: true })
	await npm(['--workspace', 'backend', 'run', 'build'])
	const directory = path.join(root, 'release')
	await rm(directory, { recursive: true, force: true })
	await mkdir(path.join(directory, 'runtime'), { recursive: true })
	for (const [from, to] of [
		['backend/dist', 'dist'],
		['backend/bluink', 'bluink'],
		['backend/LICENSE', 'LICENSE'],
		['backend/PACKAGE-README.md', 'README.md'],
		['backend/vendor', 'vendor'],
	])
		await cp(path.join(root, from), path.join(directory, to), {
			recursive: true,
		})
	await cp(
		path.join(nativeDirectory, 'index.cjs'),
		path.join(directory, 'runtime/index.cjs'),
	)
	for (const entry of native)
		await cp(
			path.join(nativeDirectory, entry.file),
			path.join(directory, 'runtime', entry.file),
		)
	const manifest = {
		name: source.name,
		version: productVersion,
		description: source.description,
		type: 'module',
		main: './dist/index.js',
		bin: source.bin,
		engines: source.engines,
		license: source.license,
		homepage: 'https://bluink.app',
		...(identity.publicRepository && {
			repository: {
				type: 'git',
				url: `${identity.publicRepository}.git`,
				directory: 'backend',
			},
		}),
		dependencies: { ...source.dependencies, '@bluink/web': productVersion },
		publishConfig: {
			access: 'public',
			tag: identity.channel === 'stable' ? 'latest' : identity.channel,
		},
		files: [...source.files, 'release.json'],
	}
	await writeJson(path.join(directory, 'package.json'), manifest)
	await writeJson(path.join(directory, 'dist/package.json'), manifest)
	await writeJson(
		path.join(directory, 'npm-shrinkwrap.json'),
		productionLock(manifest, dependencies),
	)
	await writeJson(path.join(directory, 'release.json'), {
		format: 'bluink.release',
		formatVersion: 1,
		...identity,
		name: manifest.name,
		version: manifest.version,
		distribution: hostOnly ? 'host-test' : 'all-platforms',
		contractHash: inspected.metadata.contractHash,
		dependencies: dependencies.map(({ name, version, integrity }) => ({
			name,
			version,
			integrity,
		})),
		native,
	})
	await rustNotices(path.join(directory, 'THIRD_PARTY_NOTICES.txt'))
	await chmod(path.join(directory, 'bluink'), 0o755)
	await auditDirectory(directory, {
		allowed: cliFileAllowed,
		required: [
			'bluink',
			'dist/index.js',
			'npm-shrinkwrap.json',
			'THIRD_PARTY_NOTICES.txt',
		],
	})
	console.log(`Staged ${manifest.name}@${manifest.version}.`)
	return directory
}

if (
	process.argv[1] &&
	path.resolve(process.argv[1]) === fileURLToPath(import.meta.url)
) {
	const { values } = (await import('node:util')).parseArgs({
		options: {
			version: { type: 'string' },
			dependencies: { type: 'string' },
			'host-only': { type: 'boolean', default: false },
			'native-dir': { type: 'string' },
		},
	})
	const records = await json(
		values.dependencies ??
			path.join(root, '.resource-packages/dependencies.json'),
	)
	await buildPackage({
		version: values.version,
		dependencies: records,
		hostOnly: values['host-only'],
		...(values['native-dir'] && {
			nativeDirectory: path.resolve(values['native-dir']),
		}),
	})
}
