"""Offline fake-provider shape guard; never starts a service or sends a request."""
import json


def assert_advertised(item, tools):
    if item.get('type') not in ('custom_tool_call', 'function_call'):
        return
    namespace = item.get('namespace')
    candidates = tools
    if namespace:
        containers = [t for t in tools if t.get('type') == 'namespace' and t.get('name') == namespace]
        assert len(containers) == 1, 'fixture requested an unadvertised namespace'
        candidates = containers[0]['tools']
    expected = 'custom' if item['type'] == 'custom_tool_call' else 'function'
    matches = [t for t in candidates if t.get('name') == item['name'] and t.get('type') == expected]
    assert len(matches) == 1, 'fixture requested an unadvertised tool: ' + item['name']
    if expected == 'function':
        args = json.loads(item['arguments'])
        schema = matches[0].get('parameters', {})
        assert isinstance(args, dict)
        assert set(schema.get('required', [])) <= set(args), 'fixture missing required arguments'
        if schema.get('additionalProperties') is False:
            assert set(args) <= set(schema.get('properties', {})), 'fixture supplied unknown arguments'


if __name__ == '__main__':
    import sys
    from pathlib import Path
    requests = json.loads(Path(sys.argv[1]).read_text())
    tools = requests[0]['tools']
    rejected = 0
    cases = [
        {'type':'custom_tool_call','name':'exec','input':'text(1)'},
        {'type':'function_call','namespace':'mcp__cutex_job','name':'nonexistent','arguments':'{}'},
        {'type':'function_call','namespace':'mcp__cutex_job','name':'submit','arguments':'{}'},
    ]
    for item in cases:
        try:
            assert_advertised(item, tools)
        except AssertionError:
            rejected += 1
        else:
            raise AssertionError('invalid recorded request accepted')
    assert rejected == 3
    assert_advertised({'type':'function_call','namespace':'mcp__cutex_job','name':'submit',
        'arguments':json.dumps({'actionId':'offline-only','argv':['/bin/true'],'cwd':'/private'})}, tools)
    assert_advertised({'type':'custom_tool_call','name':'exec','input':'text(1)'}, [{'type':'custom','name':'exec'}])
    print('PASS: 3 recorded-schema refusals; advertised direct MCP and synthetic custom shape accepted; no execution')
