-- The chat options page's tables (UploadChatOptions, WM_Player 0x2E): the upload's five
-- references as the client packed them (preferences::ChatOptions).
ALTER TABLE characters ADD COLUMN chat_options BLOB;
