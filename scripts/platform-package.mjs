// SPDX-License-Identifier: MPL-2.0
// Copyright (c) 2023–2026 BluInk

import { cp, mkdir, readFile, rm, writeFile } from 'node:fs/promises'
import path from 'node:path'
import { packDirectory, verifyArchive } from './package-artifacts.mjs'
import { platformFileAllowed } from './package-audit.mjs'
import {
	json,
	nativeBuildInfo,
	nativeSourceDigest,
	platformAddon,
	root,
	sha256,
	writeJson,
} from './release-utils.mjs'

/** The package npm installs only on `target`'s platform, holding its addon. */
export function platformManifest(target, { version, license, identity = {} }) {
	return {
		name: target.addonPackage,
		version,
		description: `Bluink's native addon for ${target.platform}/${target.arch}, installed by the bluink package.`,
		os: [target.platform],
		cpu: [target.arch],
		...(target.platform === 'linux' && { libc: ['glibc'] }),
		main: platformAddon,
		files: [platformAddon, 'LICENSE', 'README.md', 'THIRD_PARTY_NOTICES.txt'],
		license,
		homepage: 'https://bluink.app',
		...(identity.publicRepository && {
			repository: {
				type: 'git',
				url: `${identity.publicRepository}.git`,
				directory: 'backend/addon',
			},
		}),
		publishConfig: {
			access: 'public',
			tag:
				identity.channel === 'stable'
					? 'latest'
					: (identity.channel ?? 'alpha'),
		},
	}
}

/** Stage, pack and re-verify one platform package. The packed addon must carry
 * exactly the verified native identity, so the main package can pin it. */
export async function buildPlatformPackage(
	target,
	native,
	{ nativeDirectory, version, license, identity, notices, destination },
) {
	const staging = path.join(
		destination,
		'.staging',
		target.addonPackage.split('/')[1],
	)
	await rm(staging, { recursive: true, force: true })
	await mkdir(staging, { recursive: true })
	try {
		await cp(
			path.join(nativeDirectory, native.file),
			path.join(staging, platformAddon),
		)
		await cp(path.join(root, 'backend/LICENSE'), path.join(staging, 'LICENSE'))
		await cp(notices, path.join(staging, 'THIRD_PARTY_NOTICES.txt'))
		await writeFile(
			path.join(staging, 'README.md'),
			`# ${target.addonPackage}\n\nBluink's native addon for ${target.platform}/${target.arch}. Install [bluink](https://www.npmjs.com/package/bluink) instead; npm adds this package automatically on matching machines.\n`,
		)
		await writeJson(
			path.join(staging, 'package.json'),
			platformManifest(target, { version, license, identity }),
		)
		return await packDirectory(staging, destination, {
			allowed: platformFileAllowed,
			required: [
				platformAddon,
				'package.json',
				'LICENSE',
				'THIRD_PARTY_NOTICES.txt',
			],
			onInspect: (directory) => verifyPackedAddon(directory, native),
		})
	} finally {
		await rm(staging, { recursive: true, force: true })
	}
}

/** The addon inside an extracted platform package matches its verified build. */
export async function verifyPackedAddon(directory, native) {
	const bytes = await readFile(path.join(directory, platformAddon))
	const info = nativeBuildInfo(bytes)
	if (
		sha256(bytes) !== native.sha256 ||
		info.sourceDigest !== native.sourceDigest ||
		info.target !== native.target ||
		info.version !== native.version ||
		info.publicCommit !== native.publicCommit
	)
		throw new Error(
			`Packed addon does not match its verified build: ${native.package}`,
		)
	return info
}

/** What release.json records for one packed platform package. */
export function releaseNativeEntry(record, native) {
	return {
		name: record.name,
		version: record.version,
		integrity: record.integrity,
		sha256: record.sha256,
		addon: {
			target: native.target,
			sourceDigest: native.sourceDigest,
			mode: native.mode,
			sha256: native.sha256,
			bytes: native.bytes,
		},
	}
}

/** Verify a packed platform package as a release artifact: its exact manifest and
 * a release-mode addon built from the current sources for this release. Returns
 * the addon's identity in the shape `verifyNativeArtifacts` reports. */
export async function verifyPlatformArchive(record, target, identity) {
	const license = (await json(path.join(root, 'backend/package.json'))).license
	const expected = platformManifest(target, {
		version: identity.version,
		license,
		identity,
	})
	const digest = await nativeSourceDigest()
	const inspected = await verifyArchive(record, {
		allowed: platformFileAllowed,
		required: [
			platformAddon,
			'package.json',
			'LICENSE',
			'THIRD_PARTY_NOTICES.txt',
		],
		onInspect: async (directory, { manifest }) => {
			if (JSON.stringify(manifest) !== JSON.stringify(expected))
				throw new Error(`Unexpected platform package manifest: ${record.name}`)
			const file = path.join(directory, platformAddon)
			const bytes = await readFile(file)
			const info = nativeBuildInfo(bytes)
			if (
				info.mode !== 'release' ||
				info.target !== target.rust ||
				info.version !== identity.version ||
				info.publicCommit !== identity.publicCommit ||
				info.sourceDigest !== digest
			)
				throw new Error(
					`Platform addon has the wrong release identity: ${record.name}`,
				)
			return {
				...info,
				file: target.addon,
				package: target.addonPackage,
				sha256: sha256(bytes),
				bytes: bytes.length,
			}
		},
	})
	return inspected.inspection
}
