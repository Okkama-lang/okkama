"""Manifest-driven comparisons across incompatible ownership ABIs."""
import argparse
import hashlib
import json
import os
from pathlib import Path
import re
import tempfile

import oracle


DEFAULT_COMMANDS = {
    'build': ['{prefix}/bin/okkama', 'build', '{source}', '--target', 'sparrow', '-o', '{binary}'],
    'run': ['{prefix}/bin/okkama', 'run', '--engine', 'sparrow', '{source}'],
    'exec': ['{prefix}/bin/sparrow', '{binary}'],
}


def command_json(value):
    try:
        command = json.loads(value)
    except ValueError as exc:
        raise argparse.ArgumentTypeError(str(exc))
    if not isinstance(command, list) or not command or not all(isinstance(x, str) for x in command):
        raise argparse.ArgumentTypeError('expected a nonempty JSON argv array')
    return command


def add_arguments(parser):
    parser.add_argument('--manifest', type=Path, default=Path(__file__).with_name('p1-manifest.json'))
    parser.add_argument('--filter', action='append', default=[], help='exact manifest entry ID; repeatable')
    for side in ('a', 'b'):
        parser.add_argument(f'--{side}-toolchain', type=Path)
        parser.add_argument(f'--{side}-revision', help='required immutable source revision')
        parser.add_argument(f'--{side}-spb-version', type=int)
        for kind, default in DEFAULT_COMMANDS.items():
            parser.add_argument(f'--{side}-{kind}-command', type=command_json, default=default,
                                help='JSON argv; {prefix}, {source}, {binary}, {root} placeholders')


def load_manifest(path, root, selected):
    data = json.loads(path.read_text())
    if (not isinstance(data, dict) or data.get('schema') != 'okkama-p1-manifest/1' or
            not isinstance(data.get('entries'), list)):
        raise ValueError('expected okkama-p1-manifest/1 with entries')
    ids = []
    for entry in data['entries']:
        if not isinstance(entry, dict):
            raise ValueError('manifest entries must be objects')
        for key in ('id', 'source', 'checks', 'rule', 'features', 'checkpoints', 'failure',
                    'destroy', 'baseline', 'model_fixture', 'cleanup', 'expected', 'identities'):
            if key not in entry:
                raise ValueError(f'manifest entry missing {key}')
        if not isinstance(entry['id'], str) or not entry['id'] or entry['id'] in ids:
            raise ValueError('empty or duplicate manifest ID')
        ids.append(entry['id'])
        for key in ('source', 'rule', 'model_fixture'):
            if not isinstance(entry[key], str) or not entry[key]:
                raise ValueError(f'{key} must be a nonempty string')
        for key in ('features', 'checkpoints', 'destroy'):
            if not isinstance(entry[key], list) or not all(isinstance(x, str) for x in entry[key]):
                raise ValueError(f'{key} must be an array of strings')
        if (not isinstance(entry['checks'], list) or
                any(type(x) is not int or x < 1 for x in entry['checks']) or
                len(entry['checks']) != len(set(entry['checks']))):
            raise ValueError('checks must be unique positive integers')
        baseline = entry['baseline']
        if not isinstance(baseline, dict) or any(
                not isinstance(baseline.get(key), str) or not baseline[key]
                for key in ('revision', 'toolchain')):
            raise ValueError('baseline needs revision and toolchain strings')
        if not isinstance(entry['identities'], list):
            raise ValueError('identities must be an array')
        failure = entry['failure']
        if (not isinstance(failure, dict) or set(failure) != {'code', 'stderr'} or
                type(failure['code']) is not int or failure['code'] < 0 or
                not isinstance(failure['stderr'], str)):
            raise ValueError('failure must contain a nonnegative code and exact stderr')
        if entry['expected'] not in ('supported', 'unsupported'):
            raise ValueError('expected must be supported or unsupported')
        if entry['cleanup'] not in ('static', 'journal', 'dynamic'):
            raise ValueError('invalid cleanup path')
        for key in ('source', 'model_fixture'):
            resolved = (root / entry[key]).resolve()
            resolved.relative_to(root)
            if not resolved.is_file():
                raise ValueError(f'missing {key}: {resolved}')
        if entry['expected'] == 'unsupported':
            if not isinstance(entry.get('reason'), str) or not entry['reason']:
                raise ValueError('unsupported entry needs reason')
        else:
            if not isinstance(entry.get('stdout'), str):
                raise ValueError('supported entry needs exact stdout and failure {code, stderr}')
            identity_maps(entry)
    unknown = set(selected) - set(ids)
    if unknown:
        raise ValueError(f'unknown manifest IDs: {sorted(unknown)}')
    entries = [e for e in data['entries'] if not selected or e['id'] in selected]
    if not entries:
        raise ValueError('empty manifest selection')
    return entries


def identity_maps(entry):
    """Each diagnostic ID is scoped by ordered test block, never destroy order."""
    maps = {'A': {}, 'B': {}}
    events = set()
    for identity in entry['identities']:
        if not isinstance(identity, dict) or not all(key in identity for key in ('event', 'A', 'B')):
            raise ValueError('identity needs creation event and A/B references')
        event = identity['event']
        if not isinstance(event, str) or not event or event in events:
            raise ValueError('missing/duplicate creation-event identity')
        events.add(event)
        for side in maps:
            ref = identity[side]
            if not isinstance(ref, dict) or not all(key in ref for key in ('block', 'serial', 'type')):
                raise ValueError('identity reference needs block, serial and type')
            block, serial = ref['block'], ref['serial']
            if type(block) is not int or block < 0 or type(serial) is not int or serial < 1:
                raise ValueError('invalid block/serial identity')
            if not isinstance(ref['type'], str) or not ref['type']:
                raise ValueError('identity type must be a nonempty string')
            key = (block, serial)
            if key in maps[side]:
                raise ValueError(f'duplicate {side} identity {key}')
            maps[side][key] = (event, ref['type'])
    if len(entry['destroy']) != len(set(entry['destroy'])) or set(entry['destroy']) != events:
        raise ValueError('destroy must contain every creation-event identity exactly once')
    return maps


def mapped_trace(path, mapping):
    block = 0
    seen = set()
    output = []
    destroyed = []
    for line in Path(path).read_bytes().splitlines(keepends=True):
        if line.startswith(b'# test '):
            block += 1
            output.append(('test', line))
            continue
        match = re.fullmatch(rb'(.+) ([0-9]+)\n', line)
        if not match:
            raise ValueError(f'invalid trace line: {line!r}')
        key = (block, int(match[2]))
        if key not in mapping:
            raise ValueError(f'unmapped identity {key}')
        if key in seen:
            raise ValueError(f'duplicate identity {key}')
        event, typename = mapping[key]
        if match[1].decode('utf-8') != typename:
            raise ValueError(f'identity type mismatch {key}')
        seen.add(key)
        destroyed.append(event)
        output.append(('destroy', event))
    if seen != set(mapping):
        raise ValueError(f'missing identities: {sorted(set(mapping) - seen)}')
    return output, destroyed


def tree_hash(path):
    """Hash relative names and contents; retain individual file hashes for audit."""
    files = {str(p.relative_to(path)): oracle.fingerprint(p)
             for p in sorted(path.rglob('*')) if p.is_file()}
    digest = hashlib.sha256(json.dumps(files, sort_keys=True).encode()).hexdigest()
    return dict(sha256=digest, files=files)


def toolchains(args):
    result = {}
    for side in ('A', 'B'):
        name = side.lower()
        prefix = getattr(args, name + '_toolchain')
        revision = getattr(args, name + '_revision')
        version = getattr(args, name + '_spb_version')
        if prefix is None or not revision or version is None or version < 1:
            raise ValueError(f'{side}: toolchain, revision and positive spb-version are required')
        prefix = prefix.resolve()
        std = prefix / 'lib/okkama/std'
        if not std.is_dir():
            raise ValueError(f'missing stdlib: {std}')
        commands = {kind: getattr(args, f'{name}_{kind}_command') for kind in DEFAULT_COMMANDS}
        executables = {}
        for command in commands.values():
            executable = Path(command[0].format(prefix=prefix, root=args.root))
            if not executable.is_absolute() or not os.access(executable, os.X_OK):
                raise ValueError(f'command executable must be absolute and executable: {executable}')
            executables[str(executable)] = oracle.fingerprint(executable)
        result[side] = dict(prefix=str(prefix), revision=revision, spb_version=version,
                            stdlib=tree_hash(std), commands=commands, executables=executables)
    return result


def process(entry, args, output, chains):
    folder = output / hashlib.sha256(entry['id'].encode()).hexdigest()[:20]
    folder.mkdir()
    source = args.root / entry['source']
    result = dict(id=entry['id'], manifest=entry, artifacts=str(folder),
                  inputs={key: oracle.fingerprint(args.root / entry[key])
                          for key in ('source', 'model_fixture')},
                  builds={}, runs={}, binaries={}, differences=[], determinism_diff=[],
                  problems=[], status='same')
    if entry['expected'] == 'unsupported':
        result.update(status='unsupported', skip_reason=entry['reason'])
        return result
    maps = identity_maps(entry)
    canonical = {}
    for side in ('A', 'B'):
        chain = chains[side]
        env = {k: v for k, v in os.environ.items() if k not in oracle.INSTRUMENTATION}
        env.update(dict(getattr(args, side.lower() + '_env')))
        env.update(OKKAMA_STDLIB_DIR=str(Path(chain['prefix']) / 'lib/okkama/std'),
                   OKKAMA_IMPORT_DIRS=str(args.root / 't/support'))
        for repeat in (1, 2):
            label = side + str(repeat)
            binary = folder / (label + '.spb')
            values = dict(prefix=chain['prefix'], source=source, binary=binary, root=args.root)
            commands = {kind: [arg.format(**values) for arg in command]
                        for kind, command in chain['commands'].items()}
            build = oracle.execute(commands['build'], env, source.parent,
                                   folder / (label + '.build'), args.build_timeout)
            result['builds'][label] = build
            if not oracle.healthy(build) or build['code'] != 0 or not binary.is_file():
                result['problems'].append(label + ': unexpected build-fail or missing binary')
                continue
            with binary.open('rb') as stream:
                header = stream.read(6)
            observed_version = (int.from_bytes(header[4:6], 'big')
                                if header[:4] == b'SPRW' and len(header) == 6 else None)
            result['binaries'][label] = dict(sha256=oracle.fingerprint(binary),
                                            spb_version=observed_version)
            if observed_version != chain['spb_version']:
                result['problems'].append(label + ': unexpected SPB magic/version')
                continue
            for kind in ('run', 'exec'):
                run_label = label + '.' + kind
                trace = folder / (run_label + '.trace')
                trace.touch()
                run = oracle.execute(commands[kind], dict(env, SPARROW_TRACE_DESTROY=str(trace)),
                                     source.parent, folder / run_label, args.timeout)
                run['trace'] = str(trace)
                run['tap'] = oracle.tap_report(run)
                result['runs'][run_label] = run
                if not oracle.healthy(run) or run['code'] != entry['failure']['code']:
                    result['problems'].append(run_label + ': unexpected exit, timeout or launch failure')
                if (Path(run['stdout']).read_bytes() != entry['stdout'].encode() or
                        Path(run['stderr']).read_bytes() != entry['failure']['stderr'].encode()):
                    result['problems'].append(run_label + ': stdout/checkpoint/failure contract mismatch')
                tap = run['tap']
                if tap['failures'] or tap['problems'] or tap['skipped'] or tap['todo']:
                    result['problems'].append(run_label + ': TAP failure or unexpected skip')
                try:
                    normalized, destroyed = mapped_trace(trace, maps[side])
                    canonical[run_label] = normalized
                    if destroyed != entry['destroy']:
                        result['differences'].append(run_label + ': normative destroy order differs')
                except ValueError as exc:
                    result['problems'].append(run_label + ': ' + str(exc))
        runs = result['runs']
        first = side + '1.run'
        if first in runs:
            for label in (side + '1.exec', side + '2.run', side + '2.exec'):
                if label in runs:
                    diff = oracle.compare(runs[first], runs[label])
                    if Path(runs[first]['stderr']).read_bytes() != Path(runs[label]['stderr']).read_bytes():
                        diff['stderr'] = 'bytes differ'
                    if diff:
                        result['differences'].append(dict(left=first, right=label, raw=diff))
                        if label.startswith(side + '2'):
                            result['determinism_diff'].append(dict(left=first, right=label, raw=diff))
    if 'A1.run' in canonical and 'B1.run' in canonical and canonical['A1.run'] != canonical['B1.run']:
        result['differences'].append('cross-toolchain mapped trace differs')
    if len(result['runs']) != 8:
        result['problems'].append('selected entry has missing executions')
    if result['problems']:
        result['status'] = 'failed'
    elif result['determinism_diff']:
        result['status'] = 'nondeterministic'
    elif result['differences']:
        result['status'] = 'DIFF'
    return result


def run_suite(args):
    args.root = args.root.resolve()
    if args.toolchain or args.corpus or args.pending or args.bench or args.program_arg:
        raise ValueError('two-toolchains uses manifest selection and --a/--b-toolchain')
    entries = load_manifest(args.manifest, args.root, args.filter)
    chains = toolchains(args)
    args.out.mkdir(parents=True, exist_ok=True)
    output = Path(tempfile.mkdtemp(prefix='two-toolchains-', dir=args.out.resolve()))
    # Deliberately serial A/A/B/B: no interference between repeated invocations.
    results = [process(entry, args, output, chains) for entry in entries]
    failed = [r['id'] for r in results if r['status'] not in ('same', 'unsupported')]
    report = dict(schema=3, mode=args.mode, root=str(args.root), artifacts=str(output),
                  manifest=dict(path=str(args.manifest.resolve()), sha256=oracle.fingerprint(args.manifest),
                                selected=[e['id'] for e in entries]),
                  toolchains=chains, overrides={'A': dict(args.a_env), 'B': dict(args.b_env)},
                  settings=dict(timeout=args.timeout, build_timeout=args.build_timeout, execution='serial A/A/B/B'),
                  gate=dict(passed=not failed, failed=failed,
                            unsupported=[r['id'] for r in results if r['status'] == 'unsupported'],
                            executed=sum(r['status'] != 'unsupported' for r in results)), programs=results)
    payload = json.dumps(report, indent=2) + '\n'
    (output / 'report.json').write_text(payload)
    if args.json:
        args.json.parent.mkdir(parents=True, exist_ok=True)
        args.json.write_text(payload)
    for result in results:
        print(f'{result["status"]:16} {result["id"]}')
    print(f'Report: {output / "report.json"}')
    return int(bool(failed))
