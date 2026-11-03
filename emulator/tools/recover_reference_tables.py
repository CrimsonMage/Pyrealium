"""Catalog historical AC2 SQL and HTTrack wiki content without executing either.

Guide coordinates and names are authoring leads, never generator placements.
SQL is deduplicated by bytes; article revisions remain separate evidence.
Requires lxml to read cached HTML. Account/character tables are never exported.
"""
import argparse
from collections import Counter
import hashlib
import json
from pathlib import Path
import re
from urllib.parse import unquote, urljoin, urlsplit
import zipfile

from audit_pyreal_sql import CONTENT, GUIDE_TABLES, read_content

NAMESPACES = {'file', 'image', 'file_talk', 'image_talk', 'user', 'user_talk',
              'talk', 'template', 'template_talk', 'category', 'category_talk',
              'special', 'mediawiki', 'mediawiki_talk', 'help', 'help_talk',
              'project', 'project_talk', 'ac2wiki', 'ac2wiki_talk'}
LABELS = ('Requirements', 'Quest Starter', 'Start Location', 'Time Limit',
          'Repeat Time', 'XP Reward', 'Other Rewards', 'Related Quests', 'Rewards', 'Type')
LABEL_PATTERN = '|'.join(map(re.escape, LABELS))


def fingerprint(raw):
    return hashlib.sha256(raw).hexdigest()


def normalized(text):
    return ''.join(c for c in text.casefold() if c.isalnum())


def article_title(url):
    parts = urlsplit(url)
    if parts.hostname != 'ac2wiki.info' or parts.query or parts.fragment:
        return None
    prefix = '/index.php/'
    if not parts.path.startswith(prefix):
        return None
    title = unquote(parts.path[len(prefix):]).replace('_', ' ')
    if not title or ('/' in title) or title.split(':', 1)[0].replace(' ', '_').lower() in NAMESPACES:
        return None
    return title


def compact(element):
    return re.sub(r'\s+', ' ', ' '.join(element.itertext())).strip()


def parse_article(url, raw):
    from lxml import html
    title = article_title(url)
    if title is None or not raw:
        return None
    root = html.fromstring(raw, parser=html.HTMLParser(no_network=True))
    nodes = root.xpath('//*[@id="mw-content-text"]')
    if not nodes:
        return None
    content = nodes[0]
    if content.xpath('.//*[contains(concat(" ",normalize-space(@class)," ")," noarticletext ")]'):
        return None
    for element in content.xpath('.//script|.//style|.//*[@id="toc"]|.//*[contains(concat(" ",normalize-space(@class)," ")," mw-editsection ")]'):
        element.drop_tree()
    # Keep native article text, links and table boundaries instead of trying to
    # turn guide prose into executable quest logic or weighted loot rules.
    text = '\n'.join(t.strip() for t in content.itertext() if t.strip())
    text = text.replace('\xa0', ' ')
    flat = re.sub(r'\s+', ' ', text)
    if not flat or 'There is currently no text in this page' in flat:
        return None
    headings = [compact(h) for h in content.xpath('.//h2|.//h3|.//h4')]
    tables = []
    for table in content.xpath('.//table'):
        rows = []
        for tr in table.xpath('.//tr'):
            if tr.xpath('ancestor::table[1]')[0] is not table:
                continue
            cells = [{'text': compact(td), 'colspan': td.get('colspan', '1'),
                      'rowspan': td.get('rowspan', '1')} for td in tr.xpath('./th|./td')]
            if any(c['text'] for c in cells):
                rows.append(cells)
        if rows:
            tables.append(rows)
    links = []
    for link in content.xpath('.//a[@href]'):
        target = urljoin(url, link.get('href')).split('#', 1)[0]
        if article_title(target):
            entry = {'title': compact(link), 'url': target}
            if entry not in links:
                links.append(entry)
    overview = flat.split('Walkthrough', 1)[0]
    facts = {m[1]: m[2].strip() for m in re.finditer(
        rf'\b({LABEL_PATTERN})\s*:\s*(.*?)(?=\b(?:{LABEL_PATTERN})\s*:|$)', overview)}
    return {'title': title, 'url': url, 'html_sha256': fingerprint(raw),
            'headings': headings, 'facts': facts, 'tables': tables,
            'links': links, 'text': text}


def read_wiki(path):
    result, errors = [], []
    with zipfile.ZipFile(path) as archive:
        seen = set()
        for info in sorted(archive.infolist(), key=lambda i: (i.filename, i.header_offset)):
            if not article_title(info.filename):
                continue
            if info.file_size > 8 * 1024 * 1024:
                errors.append({'url': info.filename, 'reason': 'article_size_bound'})
                continue
            try:
                article = parse_article(info.filename, archive.read(info))
            except (ValueError, zipfile.BadZipFile) as error:
                errors.append({'url': info.filename, 'reason': str(error)})
                continue
            if article:
                identity = article['url'], article['html_sha256']
                if identity not in seen:
                    seen.add(identity)
                    result.append(article)
    return result, errors


def location_hints(row):
    """Preserve guide coordinate pairs; reject 0,0 placeholders and bad axes."""
    pairs = []
    for suffix in ('', '1', '2', '3', '4', '5'):
        north, east = row.get(f'loc{suffix}N'), row.get(f'loc{suffix}E')
        if north is None or east is None:
            continue
        n = re.fullmatch(r'\s*(\d+(?:\.\d+)?)\s*([NS])\s*', str(north), re.I)
        e = re.fullmatch(r'\s*(\d+(?:\.\d+)?)\s*([EW])\s*', str(east), re.I)
        if not n or not e or (float(n[1]) == 0 and float(e[1]) == 0):
            continue
        pairs.append({'north_south': north, 'east_west': east,
                      'precision': 'guide coordinates; no cell, height, heading or generator roster'})
    return pairs


def read_sql_corpus(roots):
    paths = sorted({path for root in roots for path in root.rglob('*.sql')})
    sources, errors = {}, []
    for path in paths:
        if path.stat().st_size > 32 * 1024 * 1024:
            errors.append({'path': path.as_posix(), 'reason': 'sql_size_bound'})
            continue
        digest = fingerprint(path.read_bytes())
        if digest in sources:
            sources[digest]['paths'].append(path.as_posix())
            continue
        try:
            tables, checked_digest = read_content(path, CONTENT | GUIDE_TABLES)
            assert checked_digest == digest
            sources[digest] = {'sha256': digest, 'paths': [path.as_posix()],
                               'tables': tables, 'row_counts': {k: len(v) for k, v in tables.items()}}
        except (ValueError, UnicodeError) as error:
            errors.append({'path': path.as_posix(), 'sha256': digest, 'reason': str(error)})
    return list(sources.values()), errors, len(paths)


def crosslinks(sql, articles):
    by_title = {}
    for article in articles:
        by_title.setdefault(normalized(article['title']), []).append(article)
    links = []
    for source in sql:
        for table, rows in source['tables'].items():
            if table not in GUIDE_TABLES:
                continue
            for index, row in enumerate(rows):
                matches = []
                for column, value in row.items():
                    if column.lower() not in {'name', 'startermob', 'guidename', 'detailfile'} or not isinstance(value, str):
                        continue
                    for article in by_title.get(normalized(value), []):
                        matches.append({'column': column, 'url': article['url'], 'html_sha256': article['html_sha256']})
                hints = location_hints(row)
                if matches or hints:
                    links.append({'source_sha256': source['sha256'], 'table': table,
                                  'row_index': index, 'article_candidates': matches, 'location_hints': hints})
    return links


def extract_incursions(articles):
    """Keep explicit starter/target counts distinct from simultaneous spawns."""
    records, conflicts, rejected = [], [], []
    for article in articles:
        if article['title'] != 'Incursion':
            continue
        for ti, table in enumerate(article['tables']):
            headers = [c['text'] for c in table[0]]
            if not {'Quest Name', 'Quest Starter', 'Quest Target', 'Repeat'} <= set(headers):
                continue
            for ri, row in enumerate(table[1:], 1):
                source = {'url': article['url'], 'html_sha256': article['html_sha256'],
                          'table_index': ti, 'row_index': ri}
                if len(row) != len(headers) or any(c['colspan'] != '1' or c['rowspan'] != '1' for c in row):
                    rejected.append({**source, 'reason': 'spanned_or_misaligned_row'})
                    continue
                values = dict(zip(headers, [c['text'] for c in row]))
                records.append({**source, 'values': values})
                for detail in articles:
                    if normalized(detail['title']) != normalized(values['Quest Name']):
                        continue
                    index_repeat, detail_repeat = duration_minutes(values['Repeat']), duration_minutes(detail['facts'].get('Repeat Time', ''))
                    if index_repeat is not None and detail_repeat is not None and index_repeat != detail_repeat:
                        conflicts.append({'quest_title': values['Quest Name'], 'field': 'repeat_minutes',
                            'index': {**source, 'value': index_repeat},
                            'detail': {'url': detail['url'], 'html_sha256': detail['html_sha256'], 'value': detail_repeat}})
    return records, conflicts, rejected


def duration_minutes(text):
    m = re.fullmatch(r'\s*(\d+)\s*(min(?:ute)?s?|hours?|days?)\s*', text, re.I)
    if not m:
        return None
    unit = m[2].lower()
    return int(m[1]) * (1440 if unit.startswith('day') else 60 if unit.startswith('hour') else 1)


def native_quest_links(quests, recovered, articles):
    """Title matches locate evidence, but never establish executable identity."""
    by_title = {}
    for article in articles:
        by_title.setdefault(normalized(article['title']), []).append(article)
    recovered_names = {c['quest']: c['text_raw'] for c in recovered.get('candidates', []) if c['field'] == 'name'}
    result = []
    for quest in quests['quests']:
        did = f"{quest['id']:08x}"
        name, origin = quest['name'].get('text'), 'installed_dat'
        if not name:
            name, origin = recovered_names.get(did), 'archived_private_string'
        if not name:
            continue
        # Comparison only: preserve the original string and grammar in the
        # source catalog. Generic Incursion names cannot identify a quest page.
        comparison = re.sub(r'\[E\]$', '', name).strip()
        if normalized(comparison) in {'incursion', 'quest', ''}:
            continue
        matches = by_title.get(normalized(comparison), [])
        if matches:
            result.append({'quest': did, 'package': quest['package'], 'name_source': origin,
                           'name_raw': name, 'article_candidates': [
                               {'url': a['url'], 'html_sha256': a['html_sha256']} for a in matches]})
    return result


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('--sql-root', type=Path, action='append', required=True)
    parser.add_argument('--wiki-cache', type=Path, required=True)
    parser.add_argument('--output', type=Path, required=True)
    parser.add_argument('--wiki-output', type=Path, required=True)
    parser.add_argument('--native-quests', type=Path)
    parser.add_argument('--recovered-quest-text', type=Path)
    args = parser.parse_args()
    sources, sql_errors, sql_files = read_sql_corpus(args.sql_root)
    articles, wiki_errors = read_wiki(args.wiki_cache)
    links = crosslinks(sources, articles)
    incursions, conflicts, rejected = extract_incursions(articles)
    native_links, native_sources = [], {}
    if args.native_quests:
        raw = args.native_quests.read_bytes()
        native_sources['quests_sha256'] = fingerprint(raw)
        quests = json.loads(raw)
        recovered = {}
        if args.recovered_quest_text:
            raw = args.recovered_quest_text.read_bytes()
            native_sources['recovered_text_sha256'] = fingerprint(raw)
            recovered = json.loads(raw)
            if recovered['native_quest_catalog_sha256'] != native_sources['quests_sha256']:
                raise ValueError('Recovered strings refer to a different native quest catalog')
        native_links = native_quest_links(quests, recovered, articles)
    elif args.recovered_quest_text:
        parser.error('--recovered-quest-text requires --native-quests')
    wiki_report = {'format': 1, 'source': args.wiki_cache.as_posix(), 'articles': articles, 'errors': wiki_errors}
    args.wiki_output.parent.mkdir(parents=True, exist_ok=True)
    wiki_raw = (json.dumps(wiki_report, indent=2, ensure_ascii=False)+'\n').encode('utf-8')
    args.wiki_output.write_bytes(wiki_raw)
    # Keep full cached articles local; a compact source index is enough to find
    # them again without committing thousands of historical wiki pages.
    report = {'format': 1, 'runtime_enabled': False, 'sql_executed': False,
        'limitations': 'Historical guide rows and article candidates only. Name matches are not template identity. Coordinates are not exact world placements. Conflicting articles are retained; no spawn counts, loot probabilities or quest flow are activated.',
        'summary': {'sql_files': sql_files, 'distinct_sql_payloads': len(sources),
                    'wiki_articles': len(articles), 'articles_with_quest_starter': sum('Quest Starter' in a['facts'] for a in articles),
                    'wiki_tables': sum(len(a['tables']) for a in articles),
                    'incursion_reference_rows': len(incursions),
                    'incursion_repeat_conflicts': len(conflicts),
                    'native_quests_with_article_candidates': len(native_links),
                    'guide_rows_with_location_hints': sum(bool(l['location_hints']) for l in links),
                    'guide_rows_with_article_candidates': sum(bool(l['article_candidates']) for l in links)},
        'sql_sources': sources, 'sql_errors': sql_errors,
        'wiki_catalog': {'path': args.wiki_output.as_posix(), 'sha256': fingerprint(wiki_raw),
                         'cache': args.wiki_cache.as_posix()},
        'article_index': [{k: a[k] for k in ('title', 'url', 'html_sha256', 'facts')} for a in articles],
        'guide_crosslinks': links, 'wiki_errors': wiki_errors,
        'native_quest_sources': native_sources, 'native_quest_crosslinks': native_links,
        'incursion_reference_rows': incursions, 'incursion_repeat_conflicts': conflicts,
        'rejected_incursion_rows': rejected,
        'selected_reference_tables': [{k: a[k] for k in ('title', 'url', 'html_sha256', 'tables')}
            for a in articles if a['title'] in {'Monster Statistics', 'Loot Jewelry', 'Tyrant Armor', 'Potions'}]}
    args.output.parent.mkdir(parents=True, exist_ok=True)
    args.output.write_text(json.dumps(report, indent=2, ensure_ascii=False)+'\n', encoding='utf-8')
    print(json.dumps(report['summary']))
    print(f'SQL errors: {len(sql_errors)}; wiki errors: {len(wiki_errors)}')


if __name__ == '__main__':
    main()
