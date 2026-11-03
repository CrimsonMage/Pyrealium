import tempfile
from pathlib import Path
import unittest
from audit_pyreal_sql import read_content, rows, CONTENT, GUIDE_TABLES


class SqlContentTest(unittest.TestCase):
    def test_multiline_dump_comments_literals_and_private_tables(self):
        with tempfile.TemporaryDirectory() as tmp:
            path=Path(tmp)/'dump.sql'
            path.write_text("""/* DROP TABLE x; */
CREATE TABLE `quest` (
 `id` int,
 `name` varchar(256)
);
insert  into `quest`(`id`,`name`) values
(1,'a; -- literal'),(2,'a''b'),(3,'a\\'b');
INSERT INTO `account` VALUES (1,'never exported');
-- comment;
""")
            tables,_=read_content(path)
            self.assertEqual(tables,{'quest':[{'id':1,'name':'a; -- literal'},{'id':2,'name':"a'b"},{'id':3,'name':"a'b"}]})

    def test_expressions_and_corrupt_rows_are_rejected(self):
        for bad in ['(1,load_file(\'/x\'))','(1,NULL); DROP TABLE x','(1,2',"(1,'missing)",'(1,1e999)']:
            with self.assertRaises(ValueError):list(rows(bad))

    def test_ac2util_unquoted_table_and_spaced_column_names(self):
        with tempfile.TemporaryDirectory() as tmp:
            path=Path(tmp)/'guide.sql'
            path.write_text("CREATE TABLE npc (`name` varchar(500), ` locN` varchar(500), ` locE` varchar(500));\n"
                            "INSERT INTO `npc` (`name`, ` locN`, ` locE`) VALUES ('Orven','44.9N','7.0W');\n"
                            "INSERT INTO useraccount VALUES ('private');")
            self.assertEqual(read_content(path)[0], {})
            self.assertEqual(read_content(path, CONTENT | GUIDE_TABLES)[0],
                             {'npc':[{'name':'Orven','locN':'44.9N','locE':'7.0W'}]})
            with self.assertRaises(ValueError):read_content(path, {'useraccount'})

    def test_insert_without_column_list_uses_inline_ddl(self):
        with tempfile.TemporaryDirectory() as tmp:
            path=Path(tmp)/'guide.sql'
            path.write_text("CREATE TABLE IF NOT EXISTS `guide`.`quests` (`name` varchar(500), ` levelGuide` varchar(500));\n"
                            "INSERT IGNORE INTO guide.quests VALUES ('Broth of Tanacha', '5');")
            self.assertEqual(read_content(path, GUIDE_TABLES)[0],
                             {'quests':[{'name':'Broth of Tanacha','levelGuide':'5'}]})

    def test_normalization_cannot_merge_duplicate_columns(self):
        with tempfile.TemporaryDirectory() as tmp:
            path=Path(tmp)/'guide.sql'
            path.write_text("INSERT INTO npc (`name`,` name`) VALUES ('a','b');")
            with self.assertRaises(ValueError):read_content(path, GUIDE_TABLES)


if __name__=='__main__':unittest.main()
