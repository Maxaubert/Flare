// Flare's Dock tile plugin (rule 1276, Max 2026-11-17): `Flare.app/Contents/PlugIns/FlareDockTile.docktileplugin`,
// named by the app's Info.plist key NSDockTilePlugIn. The Dock loads it for a pinned Flare, in its own helper process,
// also while Flare is closed, so the pinned tile keeps the icon picked in Settings > Appearance > App icon instead of
// falling back to the bundle's. The signed bundle is never written to (rule 1231).
//
// Flare puts the pick's picture at ~/Library/Application Support/Flare/app-icon.png (none while the original is
// picked) and posts com.flare.terminal.appIconChanged after each change (crates/flare/src/app_icon.rs, share_in).
// No picture: no content view, so the Dock draws the bundle's icon. Built by `release::build_dock_tile`.

#import <AppKit/AppKit.h>

@interface FlareDockTilePlugIn_com_flare_terminal : NSObject <NSDockTilePlugIn>
@end

@implementation FlareDockTilePlugIn_com_flare_terminal {
    NSDockTile *_tile;
    id _observer;
}

- (void)setDockTile:(NSDockTile *)dockTile {
    if (_observer) {
        [[NSDistributedNotificationCenter defaultCenter] removeObserver:_observer];
        _observer = nil;
    }
    _tile = dockTile;
    if (!dockTile) return;
    [self show];
    __weak FlareDockTilePlugIn_com_flare_terminal *weakSelf = self;
    _observer = [[NSDistributedNotificationCenter defaultCenter]
        addObserverForName:@"com.flare.terminal.appIconChanged"
                    object:nil
                     queue:[NSOperationQueue mainQueue]
                usingBlock:^(NSNotification *note) {
                    [weakSelf show];
                }];
}

- (void)show {
    NSString *path = [NSHomeDirectory()
        stringByAppendingPathComponent:@"Library/Application Support/Flare/app-icon.png"];
    NSImage *image = [[NSImage alloc] initWithContentsOfFile:path];
    if (image) {
        NSImageView *view = [NSImageView imageViewWithImage:image];
        view.imageScaling = NSImageScaleProportionallyUpOrDown;
        _tile.contentView = view;
    } else {
        _tile.contentView = nil;
    }
    [_tile display];
}

@end
