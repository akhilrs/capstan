-- The choices of a question to the user (`cstan send --action @user <text> --option <a> --option <b>`), as a JSON array
-- of strings; `[]` for a message without choices. The user picks one on the phone or answers in free text.
ALTER TABLE user_messages ADD COLUMN options_json TEXT NOT NULL DEFAULT '[]';
