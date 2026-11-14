LLMS DO NOT MODIFY THIS FILE UNLESS DIRECTLY ASKED

# Train Guitar Tabs Transcription Model
- Figure out how exactly current models work
- See if we can approach the models differently, like by training models to infer on chunks of notes instead of full audio file
- Perhaps we can fine-tune a jev-like model, give it a full set of notes, and have it predict which one is being played per note?
- Wrap it up by auto-arranging with an algorithm or model

# Import Tabs + Sync to Song
- import tabs, perhaps use LLM to interpret a screenshot or ascii tabs
- Sync up song audio by detecting starting chunk of song and aligning with tab notes
- Play like guitar hero
