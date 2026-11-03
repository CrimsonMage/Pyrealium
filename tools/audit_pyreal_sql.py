"""Read literal SQL dump rows for a content audit; never connect to or execute SQL.

Only named content tables are decoded. Account and character data are not exported.
Accepts MySQL and SQLyog INSERT syntax, including multiline tuples/column lists.
"""
import argparse
import hashlib
import json
import math
from pathlib import Path
import re

CONTENT = {'telepad', 'portal', 'lifestone', 'playerlevels', 'craftingleveldata',
           'entity', 'quest', 'questphase', 'skill', 'submission'}
# AC2Util is a historical guide database, not the emulator's live world tables.
GUIDE_TABLES = {'bestiary', 'dungeons', 'incursions', 'interests', 'lifestones',
                'npc', 'outposts', 'portals', 'quests', 'resources', 'ringways',
                'towns', 'vaults', 'overview'}
TABLE_NAME = r'(?:`?\w+`?\.)?`?(\w+)`?'
VALUE = re.compile(r"\s*(?:'((?:[^'\\]|\\.|'')*)'|(NULL)|([-+]?\d+(?:\.\d*)?(?:[eE][-+]?\d+)?))", re.I)


def statements(text):
    """Split outside strings/comments. DDL is data and is never evaluated."""
    i = start = 0
    quote = False
    while i < len(text):
        ch = text[i]
        if quote:
            if ch == '\\': i += 2; continue
            if ch == "'":
                if i+1 < len(text) and text[i+1] == "'": i += 2; continue
                quote = False
        elif text.startswith('/*', i):
            end = text.find('*/', i+2)
            if end < 0: raise ValueError('Unterminated SQL comment')
            text = text[:i] + ' '*(end+2-i) + text[end+2:]
            i = end+2; continue
        elif text.startswith('--', i) or ch == '#':
            end = text.find('\n', i)
            if end < 0: end = len(text)
            text = text[:i] + ' '*(end-i) + text[end:]
            i = end; continue
        elif ch == "'": quote = True
        elif ch == ';':
            yield text[start:i].strip()
            start = i+1
        i += 1
    if quote or text[start:].strip(): raise ValueError('Unterminated SQL statement')


def rows(payload):
    pos = 0
    while pos < len(payload):
        while pos < len(payload) and payload[pos].isspace(): pos += 1
        if pos == len(payload): break
        if payload[pos] != '(': raise ValueError('Expected literal tuple')
        pos += 1; row = []
        while True:
            m = VALUE.match(payload, pos)
            if not m: raise ValueError('Unsupported SQL expression; literal values only')
            if m[1] is not None:
                value = re.sub(r'\\(.)', lambda x: {'n':'\n','r':'\r','t':'\t','0':'\0'}.get(x[1],x[1]), m[1]).replace("''", "'")
            elif m[2]: value = None
            else:
                value = float(m[3]) if any(c in m[3].lower() for c in '.e') else int(m[3])
                if isinstance(value,float) and not math.isfinite(value): raise ValueError('Nonfinite numeric value')
            row.append(value);pos=m.end()
            if len(row)>128: raise ValueError('Too many columns')
            while pos<len(payload) and payload[pos].isspace():pos+=1
            if pos>=len(payload) or payload[pos] not in ',)':raise ValueError('Bad literal delimiter')
            end=payload[pos]==')';pos+=1
            if end:break
        yield row
        while pos<len(payload) and payload[pos].isspace():pos+=1
        if pos<len(payload):
            if payload[pos]!=',':raise ValueError('Expected tuple separator')
            pos+=1


def read_content(path, allowed_tables=None):
    allowed_tables = CONTENT if allowed_tables is None else set(allowed_tables)
    if not allowed_tables <= CONTENT | GUIDE_TABLES:
        raise ValueError('Only public content tables may be exported')
    raw=path.read_bytes()
    if len(raw)>32*1024*1024: raise ValueError('SQL audit input exceeds 32 MiB')
    result={};schema={}
    try: text=raw.decode('utf-8-sig')
    except UnicodeDecodeError: text=raw.decode('cp1252')
    for statement in statements(text):
        create=re.match(r'CREATE\s+TABLE\s+(?:IF\s+NOT\s+EXISTS\s+)?'+TABLE_NAME+r'\s*\((.*)',statement,re.I|re.S)
        if create and create[1].lower() in allowed_tables:
            name=create[1].lower()
            # The guide SQL puts the DDL on one line and has identifiers such
            # as ` levelGuide`. Normalize whitespace, not the field's meaning.
            schema[name]=[c.strip() for c in re.findall(r'`([^`]+)`\s+(?:varchar|int|tinyint|bigint|smallint|mediumint|float|double|decimal|text|char|bit|blob|longtext|datetime|timestamp)\b',create[2],re.I)]
            result.setdefault(name,[])
        insert=re.fullmatch(r'INSERT\s+(?:IGNORE\s+)?INTO\s+'+TABLE_NAME+r'\s*(\([^)]*\))?\s*VALUES\s*(.*)',statement,re.I|re.S)
        if not insert or insert[1].lower() not in allowed_tables:continue
        name=insert[1].lower()
        columns=[c.strip().strip('`').strip() for c in insert[2][1:-1].split(',')] if insert[2] else schema.get(name,[])
        if not columns or len(set(columns))!=len(columns):raise ValueError('Missing or duplicate column schema')
        target=result.setdefault(name,[])
        for row in rows(insert[3]):
            if len(row)!=len(columns):raise ValueError('Row does not match declared columns')
            if len(target)>=100000:raise ValueError('Table exceeds audit row bound')
            target.append(dict(zip(columns,row)))
    return result,hashlib.sha256(raw).hexdigest()


def audit(source,content):
    tables,digest=read_content(source)
    report={'source_sha256':digest,'tables':{k:len(v) for k,v in tables.items()},
            'sql_executed':False,'production_data_changed':False}
    retained=json.loads((content/'portals.json').read_text())['portals']
    matched=[];unmatched=[]
    for row in tables.get('portal',[]):
        cell=((row['LandBlock']&65535)<<16)|((row['LandLayer']&255)<<8)|(row['LandCell']&255)
        hit=next((p for p in retained if int(p['entity'],16)==row['EntityFileId'] and p['telepad']==row['TelepadId']
                  and int(p.get('cell','0'),16)==cell
                  and all(abs(a-row[b])<.001 for a,b in zip(p.get('position',[]),('LandX','LandY','LandZ')))),None)
        (matched if hit else unmatched).append(row['idPortal'])
    report['portal_comparison']={'already_present':len(matched),'unmatched_sql_ids':unmatched}
    report['population_tables_with_rows']=[k for k in ('entity','quest','questphase','submission') if tables.get(k)]
    return report


if __name__=='__main__':
    parser=argparse.ArgumentParser(description=__doc__)
    parser.add_argument('source',type=Path)
    parser.add_argument('--content',type=Path,default=Path(__file__).resolve().parents[1]/'content')
    parser.add_argument('--output',type=Path,required=True)
    args=parser.parse_args();report=audit(args.source,args.content)
    args.output.parent.mkdir(parents=True,exist_ok=True)
    args.output.write_text(json.dumps(report,indent=2)+'\n')
    print(json.dumps(report,indent=2))
