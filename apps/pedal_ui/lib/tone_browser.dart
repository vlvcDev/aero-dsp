part of 'main.dart';

/// Where a chosen tone is loaded.
enum ToneTarget { pre, amp, cabinet }

enum ToneSource { library, tone3000, favorites, recent }

const _toneBrowserPageSize = 12;

class _BrowserMemory {
  _BrowserMemory({
    required this.source,
    required this.query,
    required this.gear,
    required this.sort,
    required this.page,
    required this.verified,
    required this.calibrated,
  });

  final ToneSource source;
  final String query;
  final String? gear;
  final String? sort;
  final int page;
  final bool verified;
  final bool calibrated;
}

/// Browser state survives for the app session, per target type.
final Map<ToneTarget, _BrowserMemory> _toneBrowserMemory = {};

void resetToneBrowserMemory() => _toneBrowserMemory.clear();

/// One row of the browser: either a local capture/IR or an online tone card.
class _BrowseItem {
  const _BrowseItem.local(CatalogModel this.model) : card = null;
  const _BrowseItem.online(ToneCard this.card) : model = null;

  final CatalogModel? model;
  final ToneCard? card;

  String get key => model?.path ?? 'tone:${card!.id}';
  String get title => model?.collectionName ?? card!.title;
  String? get localPath => model?.path ?? card!.localPath;
  bool get isIr => (model?.kind ?? card!.format) == 'ir';

  String? get gear {
    final m = model;
    if (m == null) return card!.gear;
    if (m.gear != null) return m.gear;
    return switch (m.category) {
      'Pedals' => 'pedal',
      'Amps' => 'amp',
      _ => null,
    };
  }
}

String? _gearLabel(String? gear) => switch (gear?.toLowerCase()) {
      'pedal' => 'PEDAL',
      'amp' => 'AMP',
      'amp-cab' || 'full-rig' => 'FULL RIG',
      'cab' => 'CAB',
      'outboard' => 'OUTBOARD',
      final other? when other.isNotEmpty => other.toUpperCase(),
      _ => null,
    };

IconData _gearIcon(String? gear) => switch (gear?.toLowerCase()) {
      'pedal' => Icons.bolt,
      'amp' || 'amp-cab' => Icons.speaker,
      'cab' => Icons.speaker_outlined,
      _ => Icons.graphic_eq,
    };

bool _gearMatchesLocal(CatalogModel m, String? gear) => switch (gear) {
      'pedal' => m.category == 'Pedals',
      'amp' => m.category == 'Amps' && m.gear != 'amp-cab',
      'amp-cab' => m.gear == 'amp-cab',
      _ => true,
    };

String _compactCount(int n) => n >= 1000000
    ? '${(n / 1000000).toStringAsFixed(1)}M'
    : n >= 1000
        ? '${(n / 1000).toStringAsFixed(1)}k'
        : '$n';

/// Full-screen tone browser. Tapping a tone loads it into [target] straight
/// away and the browser stays open so tones can be auditioned back to back.
class ToneBrowser extends StatefulWidget {
  const ToneBrowser({
    required this.catalog,
    required this.target,
    required this.onSelectModel,
    required this.onSelectCabinet,
    this.nowPlayingPath,
    this.initialSource,
    this.onBrowseFiles,
    super.key,
  });

  final CatalogClient catalog;
  final ToneTarget target;
  final String? nowPlayingPath;
  final ToneSource? initialSource;
  final Future<void> Function(CatalogModel, ModelSlot) onSelectModel;
  final Future<void> Function(String path) onSelectCabinet;

  /// Fallback file picker for IRs that are not indexed yet (cabinet only).
  final Future<void> Function()? onBrowseFiles;

  @override
  State<ToneBrowser> createState() => _ToneBrowserState();
}

class _ToneBrowserState extends State<ToneBrowser> {
  final _search = TextEditingController();
  late ToneSource _source;
  late String _query;
  String? _gear;
  String? _sort;
  bool _verified = false;
  bool _calibrated = false;
  bool _filtersOpen = false;
  int _page = 1;

  List<CatalogModel> _local = const [];
  List<_BrowseItem> _online = const [];
  int? _totalPages;
  bool _trending = false;
  bool _hasMore = false;
  bool _loading = false;
  String? _error;
  int _seq = 0;

  bool? _connected;
  bool _t3kAvailable = true;
  String? _loadingKey;
  String? _playingPath;

  bool get _isCab => widget.target == ToneTarget.cabinet;
  String get _kind => _isCab ? 'ir' : 'nam';
  String get _defaultGear => switch (widget.target) {
        ToneTarget.pre => 'pedal',
        ToneTarget.amp => 'amp',
        ToneTarget.cabinet => 'cab',
      };
  bool get _onlineSource =>
      _source == ToneSource.tone3000 || _source == ToneSource.favorites;
  bool get _needsSignIn =>
      _onlineSource && _t3kAvailable && _connected == false;

  @override
  void initState() {
    super.initState();
    final mem = _toneBrowserMemory[widget.target];
    _source = widget.initialSource ?? mem?.source ?? ToneSource.library;
    _query = mem?.query ?? '';
    _search.text = _query;
    _gear = mem != null ? mem.gear : _defaultGear;
    _sort = mem?.sort;
    _page = mem?.page ?? 1;
    _verified = mem?.verified ?? false;
    _calibrated = mem?.calibrated ?? false;
    _playingPath = widget.nowPlayingPath;
    _fetch();
  }

  @override
  void dispose() {
    _search.dispose();
    super.dispose();
  }

  void _remember() => _toneBrowserMemory[widget.target] = _BrowserMemory(
        source: _source,
        query: _query,
        gear: _gear,
        sort: _sort,
        page: _page,
        verified: _verified,
        calibrated: _calibrated,
      );

  // ---- data -------------------------------------------------------------

  List<_BrowseItem> get _items {
    final text = _search.text.trim().toLowerCase();
    final localSource = _source != ToneSource.tone3000;
    final local = !localSource
        ? const <_BrowseItem>[]
        : _local
            .where((m) =>
                (_source != ToneSource.favorites || m.favorite) &&
                _gearMatchesLocal(m, _gear) &&
                '${m.displayName} ${m.collectionName}'
                    .toLowerCase()
                    .contains(text))
            .map(_BrowseItem.local)
            .toList();
    return [...local, if (_onlineSource) ..._online];
  }

  Future<void> _fetch() {
    final seq = ++_seq;
    setState(() {
      _loading = true;
      _error = null;
    });
    return _fetchSource(seq).catchError((Object error) {
      if (!mounted || seq != _seq) return;
      setState(() {
        _loading = false;
        _error = '$error';
      });
    });
  }

  Future<void> _fetchSource(int seq) async {
    final catalog = widget.catalog;
    List<CatalogModel>? local;
    ToneResults? results;
    var trending = false;
    if (_source != ToneSource.tone3000) {
      local = await catalog.listModelsFor(
        kind: _kind,
        sort: _source == ToneSource.recent ? 'recent' : null,
      );
      if (_source == ToneSource.recent) {
        local = local.where((m) => m.lastUsed != null).toList();
      }
    }
    if (_onlineSource) {
      try {
        final status = await catalog.tone3000Status();
        _t3kAvailable = status.available;
        _connected = status.connected;
      } catch (_) {
        if (_source == ToneSource.tone3000) rethrow;
        _t3kAvailable = false;
      }
      if (_source == ToneSource.tone3000 && !_t3kAvailable) {
        throw const CatalogException(
            'TONE3000 is not configured on this device. Your local library '
            'still works offline.');
      }
      if (_t3kAvailable) {
        final gear = _gear;
        if (_source == ToneSource.favorites) {
          if (_connected == true) {
            results = await catalog.tone3000Stream('favorited',
                query: _query,
                gear: gear,
                page: _page,
                pageSize: _toneBrowserPageSize);
          }
        } else if (_connected == false && _query.isEmpty) {
          trending = true;
          results = await catalog.tone3000Trending(gear: gear);
        } else {
          try {
            results = await catalog.tone3000Search(
              query: _query,
              gear: gear,
              sort: _sort,
              format: _kind,
              verified: _verified,
              calibrated: _calibrated,
              page: _page,
              pageSize: _toneBrowserPageSize,
            );
          } on CatalogException catch (e) {
            if (e.code != 'not_connected') rethrow;
            _connected = false;
          }
        }
      }
    }
    if (!mounted || seq != _seq) return;
    final localPaths = {...?local?.map((m) => m.path)};
    setState(() {
      if (local != null) _local = local;
      _online = [
        for (final card in results?.tones ?? const <ToneCard>[])
          if (!localPaths.contains(card.localPath) ||
              _source == ToneSource.tone3000)
            _BrowseItem.online(card),
      ];
      _trending = trending;
      _totalPages = results?.totalPages;
      _hasMore = results == null || trending
          ? false
          : results.totalPages != null
              ? _page < results.totalPages!
              : results.tones.length >= _toneBrowserPageSize;
      _loading = false;
    });
  }

  void _commitQuery() {
    _query = _search.text.trim();
    _page = 1;
    _remember();
    _fetch();
  }

  void _clearSearch() {
    _search.clear();
    _commitQuery();
  }

  void _setSource(ToneSource source) {
    if (source == _source) return;
    _source = source;
    _query = _search.text.trim();
    _page = 1;
    _remember();
    _fetch();
  }

  void _setGear(String? gear) {
    _gear = gear;
    _page = 1;
    _remember();
    _onlineSource ? _fetch() : setState(() {});
  }

  void _refine(VoidCallback change) {
    change();
    _page = 1;
    _remember();
    _fetch();
  }

  Future<void> _goPage(int page) {
    _page = page;
    _remember();
    return _fetch();
  }

  // ---- loading a tone ---------------------------------------------------

  void _snack(Object error) {
    if (!mounted) return;
    ScaffoldMessenger.of(context)
      ..hideCurrentSnackBar()
      ..showSnackBar(SnackBar(content: Text('$error')));
  }

  bool _isPlaying(_BrowseItem item) =>
      _playingPath != null && item.localPath == _playingPath;

  Future<void> _load(_BrowseItem item) async {
    if (_loadingKey != null) return;
    setState(() => _loadingKey = item.key);
    try {
      if (item.isIr != _isCab) {
        throw CatalogException(_isCab
            ? 'That is a NAM capture. Open an amp or pedal slot to use it.'
            : 'That is a cabinet IR. Open the cabinet slot to use it.');
      }
      CatalogModel model;
      if (item.model != null) {
        model = item.model!;
      } else {
        final result = await widget.catalog.tone3000Load(item.card!.id);
        model = result.model;
        if ((result.kind == 'ir') != _isCab) {
          throw const CatalogException(
              'That tone does not match this slot (NAM vs IR).');
        }
      }
      if (_isCab) {
        await widget.onSelectCabinet(model.path);
      } else {
        await widget.onSelectModel(model,
            widget.target == ToneTarget.pre ? ModelSlot.pre : ModelSlot.amp);
      }
      try {
        await widget.catalog.touchModel(model.path);
      } catch (_) {}
      if (!mounted) return;
      setState(() {
        _playingPath = model.path;
        if (item.card != null) {
          _online = [
            for (final o in _online)
              o.key == item.key
                  ? _BrowseItem.online(o.card!.copyWith(localPath: model.path))
                  : o,
          ];
        }
      });
    } catch (error) {
      _snack(error);
    } finally {
      if (mounted) setState(() => _loadingKey = null);
    }
  }

  Future<void> _step(int dir) async {
    if (_loadingKey != null || _loading) return;
    var items = _items;
    final current = items.indexWhere(_isPlaying);
    var next = current < 0 ? (dir > 0 ? 0 : items.length - 1) : current + dir;
    if (next < 0 || next >= items.length) {
      if (!_onlineSource) return;
      if (dir > 0 && _hasMore) {
        await _goPage(_page + 1);
      } else if (dir < 0 && _page > 1) {
        await _goPage(_page - 1);
      } else {
        return;
      }
      if (!mounted) return;
      items = _items;
      if (items.isEmpty) return;
      next = dir > 0 ? 0 : items.length - 1;
    }
    await _load(items[next]);
  }

  Future<void> _toggleFavorite(_BrowseItem item) async {
    try {
      if (item.model != null) {
        final value = !item.model!.favorite;
        final saved = await widget.catalog.setFavorite(item.model!.path, value);
        if (!mounted) return;
        setState(() => _local = [
              for (final m in _local)
                m.path == item.model!.path ? m.withFavorite(saved) : m,
            ]);
      } else {
        final value = !(item.card!.favorited ?? false);
        final saved =
            await widget.catalog.tone3000Favorite(item.card!.id, value);
        if (!mounted) return;
        setState(() => _online = [
              for (final o in _online)
                o.key == item.key
                    ? _BrowseItem.online(o.card!.copyWith(favorited: saved))
                    : o,
            ]);
      }
    } catch (error) {
      _snack(error);
    }
  }

  // ---- UI ---------------------------------------------------------------

  Widget _chip(String label, bool selected, VoidCallback? onTap,
      {IconData? icon}) {
    return Padding(
      padding: const EdgeInsets.only(right: 6),
      child: ChoiceChip(
        avatar: icon == null ? null : Icon(icon, size: 14),
        label: Text(label,
            style: const TextStyle(fontSize: 11, fontWeight: FontWeight.w800)),
        selected: selected,
        selectedColor: LostChrome.ocean,
        visualDensity: VisualDensity.compact,
        materialTapTargetSize: MaterialTapTargetSize.shrinkWrap,
        labelPadding: const EdgeInsets.symmetric(horizontal: 2),
        onSelected: onTap == null ? null : (_) => onTap(),
      ),
    );
  }

  Widget _chipRow(List<Widget> children) => SizedBox(
        height: 34,
        child: ListView(
          scrollDirection: Axis.horizontal,
          padding: const EdgeInsets.symmetric(horizontal: 12),
          children: children,
        ),
      );

  @override
  Widget build(BuildContext context) {
    final gears = _isCab
        ? const [('CAB', 'cab')]
        : const [('PEDAL', 'pedal'), ('AMP', 'amp'), ('FULL RIG', 'amp-cab')];
    final paged = _onlineSource &&
        !_trending &&
        !_needsSignIn &&
        (_page > 1 || _hasMore || (_totalPages ?? 1) > 1);
    return Scaffold(
      backgroundColor: LostChrome.midnight,
      body: DecoratedBox(
        decoration: const BoxDecoration(
          gradient: LinearGradient(
            begin: Alignment.topCenter,
            end: Alignment.bottomCenter,
            colors: [LostChrome.navy, LostChrome.midnight],
          ),
        ),
        child: SafeArea(
          child: Column(
            children: [
              Padding(
                padding: const EdgeInsets.fromLTRB(4, 4, 4, 2),
                child: Row(
                  children: [
                    TextButton(
                      onPressed: () => Navigator.maybePop(context),
                      child: const Text('DONE',
                          style: TextStyle(fontWeight: FontWeight.w900)),
                    ),
                    Expanded(
                      child: CallbackShortcuts(
                        bindings: {
                          const SingleActivator(LogicalKeyboardKey.escape):
                              _clearSearch,
                        },
                        child: TextField(
                          controller: _search,
                          textInputAction: TextInputAction.search,
                          onChanged: (_) => setState(() {}),
                          onSubmitted: (_) => _commitQuery(),
                          decoration: InputDecoration(
                            hintText: switch (widget.target) {
                              ToneTarget.pre => 'Find a pedal…',
                              ToneTarget.amp => 'Find an amp or capture…',
                              ToneTarget.cabinet => 'Find a cabinet IR…',
                            },
                            prefixIcon: const Icon(Icons.search, size: 20),
                            suffixIcon: _search.text.isEmpty
                                ? null
                                : IconButton(
                                    tooltip: 'Clear search',
                                    icon: const Icon(Icons.close, size: 18),
                                    onPressed: _clearSearch,
                                  ),
                            isDense: true,
                          ),
                        ),
                      ),
                    ),
                    IconButton(
                      tooltip: 'Previous tone',
                      onPressed: () => _step(-1),
                      icon: const Icon(Icons.skip_previous),
                    ),
                    IconButton(
                      tooltip: 'Next tone',
                      onPressed: () => _step(1),
                      icon: const Icon(Icons.skip_next),
                    ),
                  ],
                ),
              ),
              _chipRow([
                _chip('MY LIBRARY', _source == ToneSource.library,
                    () => _setSource(ToneSource.library),
                    icon: Icons.offline_pin_outlined),
                _chip('TONE3000', _source == ToneSource.tone3000,
                    () => _setSource(ToneSource.tone3000),
                    icon: Icons.cloud_outlined),
                _chip('FAVORITES', _source == ToneSource.favorites,
                    () => _setSource(ToneSource.favorites),
                    icon: Icons.star_outline),
                _chip('RECENT', _source == ToneSource.recent,
                    () => _setSource(ToneSource.recent),
                    icon: Icons.history),
              ]),
              _chipRow([
                for (final (label, gear) in gears)
                  _chip(label, _gear == gear,
                      () => _setGear(_gear == gear ? null : gear),
                      icon: _gearIcon(gear)),
                if (_source == ToneSource.tone3000) ...[
                  _chip(_query.isEmpty ? 'TRENDING' : 'BEST MATCH',
                      _sort == null, () => _refine(() => _sort = null),
                      icon: Icons.local_fire_department_outlined),
                  _chip('DOWNLOADS', _sort == 'downloads-all-time',
                      () => _refine(() => _sort = 'downloads-all-time')),
                  _chip('NEWEST', _sort == 'newest',
                      () => _refine(() => _sort = 'newest')),
                  _chip('FILTERS', _filtersOpen || _verified || _calibrated,
                      () => setState(() => _filtersOpen = !_filtersOpen),
                      icon: Icons.tune),
                ],
                if (_isCab &&
                    _source == ToneSource.library &&
                    widget.onBrowseFiles != null)
                  ActionChip(
                    avatar: const Icon(Icons.folder_open, size: 14),
                    label: const Text('BROWSE FILES…',
                        style: TextStyle(
                            fontSize: 11, fontWeight: FontWeight.w800)),
                    visualDensity: VisualDensity.compact,
                    materialTapTargetSize: MaterialTapTargetSize.shrinkWrap,
                    onPressed: () async {
                      await widget.onBrowseFiles!();
                      if (mounted) _fetch();
                    },
                  ),
              ]),
              if (_filtersOpen && _source == ToneSource.tone3000)
                _chipRow([
                  _chip('VERIFIED CREATORS', _verified,
                      () => _refine(() => _verified = !_verified),
                      icon: Icons.verified_outlined),
                  _chip('CALIBRATED', _calibrated,
                      () => _refine(() => _calibrated = !_calibrated),
                      icon: Icons.straighten),
                ]),
              SizedBox(
                height: 2,
                child: _loading || _loadingKey != null
                    ? const LinearProgressIndicator(minHeight: 2)
                    : null,
              ),
              Expanded(child: _body()),
              if (paged) _paginator(),
            ],
          ),
        ),
      ),
    );
  }

  Widget _paginator() => Container(
        height: 40,
        color: LostChrome.glass.withValues(alpha: .6),
        child: Row(
          mainAxisAlignment: MainAxisAlignment.center,
          children: [
            IconButton(
              tooltip: 'Previous page',
              onPressed:
                  _page > 1 && !_loading ? () => _goPage(_page - 1) : null,
              icon: const Icon(Icons.chevron_left),
            ),
            Text(
              _totalPages == null
                  ? 'PAGE $_page'
                  : 'PAGE $_page / $_totalPages',
              style: const TextStyle(
                  fontWeight: FontWeight.w900, letterSpacing: .6),
            ),
            IconButton(
              tooltip: 'Next page',
              onPressed:
                  _hasMore && !_loading ? () => _goPage(_page + 1) : null,
              icon: const Icon(Icons.chevron_right),
            ),
          ],
        ),
      );

  Widget _body() {
    final items = _items;
    final empty = items.isEmpty && !_loading;
    return LayoutBuilder(builder: (context, constraints) {
      final columns = constraints.maxWidth < 560
          ? 1
          : constraints.maxWidth < 900
              ? 2
              : 3;
      return AnimatedOpacity(
        duration: const Duration(milliseconds: 120),
        opacity: _loading && _onlineSource ? .45 : 1,
        child: CustomScrollView(
          slivers: [
            if (_needsSignIn)
              SliverPadding(
                padding: const EdgeInsets.fromLTRB(12, 4, 12, 4),
                sliver: SliverToBoxAdapter(
                  child: _DeviceSignIn(
                    catalog: widget.catalog,
                    onConnected: () {
                      _connected = true;
                      _fetch();
                    },
                  ),
                ),
              ),
            if (_error != null)
              SliverFillRemaining(
                hasScrollBody: false,
                child: _message(Icons.cloud_off_outlined, _error!,
                    action: ('RETRY', _fetch)),
              )
            else if (empty && !_needsSignIn)
              SliverFillRemaining(
                hasScrollBody: false,
                child: _message(
                  Icons.search_off,
                  switch (_source) {
                    ToneSource.library => 'No matching tones on this device.',
                    ToneSource.recent => 'Nothing played from here yet.',
                    ToneSource.favorites => 'No favorites yet. Tap a star.',
                    ToneSource.tone3000 => 'No tones found.',
                  },
                ),
              )
            else
              SliverPadding(
                padding: const EdgeInsets.fromLTRB(12, 4, 12, 12),
                sliver: SliverGrid(
                  gridDelegate: SliverGridDelegateWithFixedCrossAxisCount(
                    crossAxisCount: columns,
                    mainAxisExtent: 80,
                    crossAxisSpacing: 8,
                    mainAxisSpacing: 8,
                  ),
                  delegate: SliverChildBuilderDelegate(
                    (context, index) {
                      final item = items[index];
                      final isFav =
                          item.model?.favorite ?? item.card?.favorited;
                      return _ToneTile(
                        item: item,
                        playing: _isPlaying(item),
                        loading: _loadingKey == item.key,
                        favorite: isFav,
                        showFavorite: item.model != null ||
                            (_connected == true && isFav != null),
                        onTap: _loadingKey == null ? () => _load(item) : null,
                        onFavorite: () => _toggleFavorite(item),
                      );
                    },
                    childCount: items.length,
                  ),
                ),
              ),
          ],
        ),
      );
    });
  }

  Widget _message(IconData icon, String text,
          {(String, VoidCallback)? action}) =>
      Center(
        child: Padding(
          padding: const EdgeInsets.all(16),
          child: Column(
            mainAxisSize: MainAxisSize.min,
            children: [
              Icon(icon, size: 32, color: LostChrome.ice),
              const SizedBox(height: 8),
              Text(text, textAlign: TextAlign.center),
              if (action != null) ...[
                const SizedBox(height: 8),
                OutlinedButton(onPressed: action.$2, child: Text(action.$1)),
              ],
            ],
          ),
        ),
      );
}

class _ToneTile extends StatelessWidget {
  const _ToneTile({
    required this.item,
    required this.playing,
    required this.loading,
    required this.favorite,
    required this.showFavorite,
    required this.onTap,
    required this.onFavorite,
  });

  final _BrowseItem item;
  final bool playing;
  final bool loading;
  final bool? favorite;
  final bool showFavorite;
  final VoidCallback? onTap;
  final VoidCallback onFavorite;

  @override
  Widget build(BuildContext context) {
    final model = item.model;
    final card = item.card;
    final gear = _gearLabel(item.gear);
    final meta = <String>[
      if (card?.downloads != null)
        '${_compactCount(card!.downloads!)} downloads',
      if (card?.creatorName != null) card!.creatorName!,
      if (model != null) model.sizeLabel,
    ].join(' · ');
    final image = card?.imageUrl;
    Widget thumb = Container(
      width: 56,
      height: 56,
      color: LostChrome.glass,
      child: Icon(_gearIcon(item.gear), color: LostChrome.ice, size: 28),
    );
    if (image != null) {
      thumb = Image.network(
        image,
        width: 56,
        height: 56,
        fit: BoxFit.cover,
        errorBuilder: (_, __, ___) => thumb,
      );
    }
    return Material(
      color: Colors.transparent,
      child: Ink(
        decoration: BoxDecoration(
          borderRadius: BorderRadius.circular(16),
          border: Border.all(
              color: playing
                  ? LostChrome.lime
                  : LostChrome.ice.withValues(alpha: .5)),
          gradient: const LinearGradient(
            begin: Alignment.topLeft,
            end: Alignment.bottomRight,
            colors: [Color(0xff22649a), Color(0xff102c53), Color(0xff081a36)],
          ),
        ),
        child: InkWell(
          borderRadius: BorderRadius.circular(16),
          onTap: onTap,
          child: Padding(
            padding: const EdgeInsets.all(8),
            child: Row(
              children: [
                ClipRRect(
                    borderRadius: BorderRadius.circular(10), child: thumb),
                const SizedBox(width: 10),
                Expanded(
                  child: Column(
                    crossAxisAlignment: CrossAxisAlignment.start,
                    mainAxisAlignment: MainAxisAlignment.center,
                    children: [
                      Text(item.title,
                          maxLines: 1,
                          overflow: TextOverflow.ellipsis,
                          style: const TextStyle(
                              fontWeight: FontWeight.w900, fontSize: 15)),
                      const SizedBox(height: 3),
                      ClipRect(
                        child: SingleChildScrollView(
                          scrollDirection: Axis.horizontal,
                          physics: const NeverScrollableScrollPhysics(),
                          child: Row(children: [
                            if (playing) ...[
                              const _CaptureBadge(
                                  label: 'NOW PLAYING',
                                  color: LostChrome.lime,
                                  tooltip: 'Loaded in this slot'),
                              const SizedBox(width: 4),
                            ],
                            if (gear != null) ...[
                              _CaptureBadge(
                                  label: gear,
                                  color: LostChrome.ice,
                                  tooltip: 'Gear type'),
                              const SizedBox(width: 4),
                            ],
                            _CaptureBadge(
                                label: item.isIr ? 'IR' : 'NAM',
                                color: LostChrome.electric,
                                tooltip: item.isIr
                                    ? 'Impulse response'
                                    : 'Neural amp model'),
                            if (model?.gainDb != null) ...[
                              const SizedBox(width: 4),
                              _CaptureGainBadge(gainDb: model!.gainDb!),
                            ],
                            if (model?.includesCab == true) ...[
                              const SizedBox(width: 4),
                              const _CaptureCabBadge(),
                            ],
                            if (card?.localPath != null) ...[
                              const SizedBox(width: 4),
                              const _CaptureBadge(
                                  label: 'IN LIBRARY',
                                  color: LostChrome.lime,
                                  tooltip: 'Already downloaded'),
                            ],
                          ]),
                        ),
                      ),
                      if (meta.isNotEmpty || (card?.verified ?? false)) ...[
                        const SizedBox(height: 3),
                        Row(children: [
                          Flexible(
                            child: Text(meta,
                                maxLines: 1,
                                overflow: TextOverflow.ellipsis,
                                style: const TextStyle(
                                    color: LostChrome.ice, fontSize: 11)),
                          ),
                          if (card?.verified ?? false)
                            const Padding(
                              padding: EdgeInsets.only(left: 3),
                              child: Tooltip(
                                message: 'Verified creator',
                                child: Icon(Icons.verified,
                                    size: 12, color: LostChrome.electric),
                              ),
                            ),
                        ]),
                      ],
                    ],
                  ),
                ),
                if (loading)
                  const Padding(
                    padding: EdgeInsets.all(8),
                    child: SizedBox.square(
                        dimension: 20,
                        child: CircularProgressIndicator(strokeWidth: 2)),
                  )
                else if (showFavorite)
                  IconButton(
                    tooltip:
                        favorite == true ? 'Remove favorite' : 'Add favorite',
                    visualDensity: VisualDensity.compact,
                    onPressed: onFavorite,
                    icon: Icon(
                      favorite == true ? Icons.star : Icons.star_outline,
                      color:
                          favorite == true ? LostChrome.lime : LostChrome.ice,
                    ),
                  ),
              ],
            ),
          ),
        ),
      ),
    );
  }
}

/// Compact device-code sign-in: shows the code and polls until connected.
class _DeviceSignIn extends StatefulWidget {
  const _DeviceSignIn({required this.catalog, required this.onConnected});

  final CatalogClient catalog;
  final VoidCallback onConnected;

  @override
  State<_DeviceSignIn> createState() => _DeviceSignInState();
}

class _DeviceSignInState extends State<_DeviceSignIn> {
  DeviceAuth? _auth;
  bool _busy = false;
  String? _error;
  Timer? _poll;
  DateTime? _deadline;

  @override
  void dispose() {
    _poll?.cancel();
    super.dispose();
  }

  Future<void> _begin() async {
    setState(() {
      _busy = true;
      _error = null;
    });
    try {
      final auth = await widget.catalog.beginTone3000DeviceAuth();
      if (!mounted) return;
      _deadline = DateTime.now().add(Duration(seconds: auth.expiresIn));
      _poll?.cancel();
      _poll = Timer.periodic(
          Duration(seconds: auth.interval < 1 ? 1 : auth.interval),
          (_) => _check());
      setState(() => _auth = auth);
    } catch (error) {
      if (mounted) setState(() => _error = '$error');
    } finally {
      if (mounted) setState(() => _busy = false);
    }
  }

  Future<void> _check() async {
    if (_deadline != null && DateTime.now().isAfter(_deadline!)) {
      _poll?.cancel();
      if (mounted) {
        setState(() {
          _auth = null;
          _error = 'The code expired. Try again.';
        });
      }
      return;
    }
    try {
      final status = await widget.catalog.tone3000Status();
      if (status.connected && mounted) {
        _poll?.cancel();
        widget.onConnected();
      }
    } catch (_) {}
  }

  @override
  Widget build(BuildContext context) {
    final auth = _auth;
    return Container(
      padding: const EdgeInsets.all(10),
      decoration: BoxDecoration(
        color: LostChrome.glass,
        borderRadius: BorderRadius.circular(16),
        border: Border.all(color: LostChrome.electric.withValues(alpha: .6)),
      ),
      child: auth == null
          ? Row(children: [
              const Icon(Icons.login, color: LostChrome.ice),
              const SizedBox(width: 10),
              Expanded(
                child: Text(
                  _error ??
                      'Sign in to search all of TONE3000 and sync favorites.',
                  style: const TextStyle(fontSize: 12),
                ),
              ),
              FilledButton(
                onPressed: _busy ? null : _begin,
                child: const Text('SIGN IN'),
              ),
            ])
          : Row(children: [
              Expanded(
                child: Column(
                  crossAxisAlignment: CrossAxisAlignment.start,
                  children: [
                    const Text('Open this address and enter the code:',
                        style: TextStyle(fontSize: 11)),
                    SelectableText(
                      auth.verificationUri,
                      style: const TextStyle(
                          color: LostChrome.electric,
                          fontWeight: FontWeight.w800,
                          fontSize: 12),
                    ),
                  ],
                ),
              ),
              SelectableText(
                auth.userCode,
                style: const TextStyle(
                    fontSize: 26,
                    fontWeight: FontWeight.w900,
                    letterSpacing: 3,
                    color: LostChrome.lime),
              ),
              IconButton(
                tooltip: 'Copy code',
                icon: const Icon(Icons.copy, size: 18),
                onPressed: () =>
                    Clipboard.setData(ClipboardData(text: auth.userCode)),
              ),
              const SizedBox.square(
                  dimension: 16,
                  child: CircularProgressIndicator(strokeWidth: 2)),
            ]),
    );
  }
}
