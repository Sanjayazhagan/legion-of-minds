-- Brain-size categories (see godcore::ppo::BRACKETS): a base is in the
-- category of its biggest defender brain, and matchmaking only pairs an
-- army with bases of the category it asks for. 0 = up to 256 neurons,
-- 1 = up to 1024. Numbered 0009 because 0008 is taken by the troop
-- rest table in the human-limits change.
ALTER TABLE snapshots ADD COLUMN bracket SMALLINT NOT NULL DEFAULT 0;
-- The weekly cup is ranked and paid per category.
ALTER TABLE cup_entries ADD COLUMN category SMALLINT NOT NULL DEFAULT 0;
