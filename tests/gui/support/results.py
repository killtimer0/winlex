"""Retain all case outcomes when a blocked pytest worker must be replaced."""
import json
import xml.etree.ElementTree as ET


def records(path):
    if not path.exists():
        return []
    # A killed worker may leave its final write incomplete, including a split
    # UTF-8 code point. Only newline-terminated records have been committed.
    return [json.loads(line) for line in path.read_bytes().splitlines(keepends=True)
            if line.endswith(b'\n') and line.strip()]


def merge(attempts, output):
    cases = {}
    timed_out_cases = set()
    for folder, result in attempts:
        for record in records(folder / 'progress.jsonl'):
            cases.setdefault(record['nodeid'], [])
        for record in records(folder / 'timings.jsonl'):
            cases.setdefault(record['nodeid'], []).append(record)
        if result.get('timeout'):
            timeout = result['timeout']
            timed_out_cases.add(timeout['nodeid'])
            cases.setdefault(timeout['nodeid'], []).append({
                'phase': timeout['phase'], 'outcome': 'failed', 'duration': timeout['budget'],
                'message': 'Operation timed out: ' + timeout['operation'] + '\nStack: ' +
                           str(folder / 'timeout-stacks.log'),
            })
    suite = ET.Element('testsuite', name='pytest')
    counts = dict(tests=len(cases), failures=0, errors=0, skipped=0)
    for nodeid, phases in cases.items():
        if nodeid not in timed_out_cases and not any(row['phase'] == 'teardown' for row in phases):
            # Cancellation, a whole-run budget or a worker crash can interrupt
            # a later attempt. A setup/call record alone is not a passed case.
            phases.append({'phase': 'teardown', 'outcome': 'failed', 'duration': 0,
                           'message': 'Worker interrupted before case completion; see run.json'})
        classname, _, name = nodeid.rpartition('::')
        classname = classname.replace('/', '.').replace('\\', '.')
        if classname.endswith('.py'):
            classname = classname[:-3]
        case = ET.SubElement(suite, 'testcase', classname=classname, name=name,
                             time=str(sum(row['duration'] for row in phases)))
        failed = False
        kinds = set()
        for row in phases:
            if row['outcome'] == 'failed':
                failed = True
                kind = 'failure' if row['phase'] == 'call' else 'error'
                if kind not in kinds:
                    counts['failures' if kind == 'failure' else 'errors'] += 1
                    kinds.add(kind)
                ET.SubElement(case, kind, message=row.get('message', '')).text = row.get('message', '')
        if not failed:
            if any(row['outcome'] == 'skipped' for row in phases):
                counts['skipped'] += 1
                ET.SubElement(case, 'skipped')
    suite.attrib.update({key: str(value) for key, value in counts.items()})
    root = ET.Element('testsuites')
    root.append(suite)
    ET.ElementTree(root).write(str(output / 'tests.xml'), encoding='utf-8', xml_declaration=True)
    return counts
