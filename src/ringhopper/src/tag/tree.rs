use std::borrow::Cow;
use std::collections::{HashMap, VecDeque};
use std::fs::{read, write};
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};
use primitives::error::{Error, RinghopperResult};
use primitives::primitive::{HALO_PATH_SEPARATOR, TagGroup, TagPath};
use primitives::tag::{ParseStrictness, PrimaryTagStructDyn};

/// Tag tree implementation for traversing and loading/saving tags.
pub trait TagTree {
    /// Get a copy of the tag in the tag tree if it exists.
    ///
    /// Returns `Err` if it does not exist.
    fn open_tag_copy(&self, path: &TagPath) -> RinghopperResult<Box<dyn PrimaryTagStructDyn>>;

    /// Open the tag, getting a thread-safe, potentially shared version of the tag.
    ///
    /// For tag trees that implement caching, this can return a direct reference to the in-cache version of the tag,
    /// preventing an extra copy.
    ///
    /// If this is not overridden, a copy will be returned, instead.
    fn open_tag_shared(&self, path: &TagPath) -> RinghopperResult<Arc<Mutex<Box<dyn PrimaryTagStructDyn>>>> {
        self.open_tag_copy(path).map(|b| Arc::new(Mutex::new(b)))
    }

    /// Get all files in the path.
    ///
    /// Returns `None` if the path does not exist.
    fn files_in_path(&self, path: &str) -> Option<Vec<TagTreeItem>>;

    /// Write the tag into the tag tree.
    fn write_tag(&mut self, path: &TagPath, tag: &dyn PrimaryTagStructDyn) -> RinghopperResult<()>;

    /// Get the root tag tree item.
    fn root(&self) -> TagTreeItem where Self: Sized {
        TagTreeItem::new(TagTreeItemType::Directory, Cow::default(), None, self)
    }
}

/// Tag filter
///
/// This allows you to match groups with wildcard expressions.
///
/// # Wildcards:
/// - `*` matches any number of characters, including no characters
/// - `?` matches any one character
///
/// # Examples:
/// - `*` matches anything as a catch-all
/// - `*.bitmap` matches any bitmap if `group` is unset (if group is set, it matches any `.bitmap.<group>`)
pub struct TagFilter {
    filter: String,
    group: Option<TagGroup>
}

impl TagFilter {
    /// Check if the given path is likely a filter.
    pub fn is_filter(path: &str) -> bool {
        path.chars().any(|c| c == '?' || c == '*')
    }

    /// Create a tag filter.
    ///
    /// If `group` is None, then the filter matches the whole path including group. Otherwise, the filter matches only
    /// the path, while the group matches the group.
    pub fn new(filter: &str, group: Option<TagGroup>) -> Self {
        let mut fixed = String::with_capacity(filter.len());
        for c in filter.chars() {
            if std::path::is_separator(c) {
                fixed.push(HALO_PATH_SEPARATOR)
            }
            else {
                fixed.push(c)
            }
        }
        Self {
            filter: fixed,
            group
        }
    }

    /// Test that the path passes the filter.
    ///
    /// # Examples
    ///
    /// ```
    /// use ringhopper::tag::tree::TagFilter;
    /// use ringhopper::primitives::primitive::{TagPath, TagGroup};
    ///
    /// let all_bitmaps = TagFilter::new("*.bitmap", None);
    /// assert!(all_bitmaps.passes(&TagPath::from_path("something.bitmap").unwrap()));
    /// assert!(!all_bitmaps.passes(&TagPath::from_path("something.weapon").unwrap()));
    ///
    /// let all_bitmaps = TagFilter::new("*", Some(TagGroup::Bitmap));
    /// assert!(all_bitmaps.passes(&TagPath::from_path("something.bitmap").unwrap()));
    ///
    /// let all_some_bitmaps = TagFilter::new("some*", Some(TagGroup::Bitmap));
    /// assert!(all_some_bitmaps.passes(&TagPath::from_path("something.bitmap").unwrap()));
    /// assert!(!all_some_bitmaps.passes(&TagPath::from_path("nothing.bitmap").unwrap()));
    /// assert!(!all_some_bitmaps.passes(&TagPath::from_path("something.weapon").unwrap()));
    ///
    /// let campaign_maps = TagFilter::new("levels\\???\\???", Some(TagGroup::Scenario));
    /// assert!(campaign_maps.passes(&TagPath::from_path("levels\\a10\\a10.scenario").unwrap()));
    /// assert!(!campaign_maps.passes(&TagPath::from_path("levels\\test\\wizard\\wizard.scenario").unwrap()));
    /// ```
    pub fn passes(&self, path: &TagPath) -> bool {
        if let Some(n) = self.group {
            if path.group() != n {
                return false
            }
            Self::filter_passes_raw(&self.filter, path.path())
        }
        else {
            Self::filter_passes_raw(&self.filter, &path.to_internal_path())
        }
    }

    fn filter_passes_raw(mut filter: &str, mut what: &str) -> bool {
        loop {
            let filter_first = filter.chars().next();
            let what_first = what.chars().next();

            if filter_first.is_none() && what_first.is_none() {
                return true
            }
            else if filter_first.is_none() || what_first.is_none() {
                return false
            }

            let filter_first = filter_first.unwrap();
            let what_first = what_first.unwrap();

            filter = &filter[1..];
            what = &what[1..];

            if filter_first == '?' || filter_first == what_first {
                continue
            }
            else if filter_first == '*' {
                while filter.chars().next() == Some('*') {
                    filter = &filter[1..]
                }
                if filter.is_empty() {
                    return true
                }
                while !what.is_empty() {
                    if Self::filter_passes_raw(filter, what) {
                        return true
                    }
                    what = &what[1..];
                }
                return false
            }
            else {
                return false
            }
        }
    }
}

pub struct TagTreeTagIterator<'a> {
    stack: Vec<VecDeque<TagTreeItem<'a>>>,
    filter: Option<TagFilter>
}

pub fn iterate_through_all_tags<T: TagTree>(what: &T, filter: Option<TagFilter>) -> TagTreeTagIterator {
    let mut iterator = TagTreeTagIterator {
        stack: vec![],
        filter
    };

    if let Some(n) = what.root().files() {
        iterator.stack.push(n.into())
    }

    iterator
}

impl<'a> Iterator for TagTreeTagIterator<'a> {
    type Item = TagPath;

    fn next(&mut self) -> Option<Self::Item> {
        loop {
            let last = self.stack.last_mut()?;
            let first = match last.pop_front() {
                Some(n) => n,
                None => {
                    self.stack.pop();
                    continue
                }
            };

            let found = match first.item_type {
                TagTreeItemType::Tag => first.tag_path().unwrap_or_else(|| panic!("found a tag in the tag tree tag iterator, but it does not have a TagPath")),
                TagTreeItemType::Directory => {
                    if let Some(n) = first.files() {
                        self.stack.push(n.into());
                    }
                    continue
                }
            };

            if let Some(n) = &self.filter {
                if !n.passes(&found) {
                    continue
                }
            }

            return Some(found)
        }
    }
}

/// Denotes an item type for identifying a [`TagTreeItem`].
#[derive(Copy, Clone, PartialEq)]
pub enum TagTreeItemType {
    /// The item represents a tag or tag file.
    ///
    /// Note that, in some cases, the validity of this being an actual tag and not just a file that happens to have a
    /// tag extension is not guaranteed.
    Tag,

    /// The item represents a directory that can be further traversed.
    Directory
}

/// Denotes a tag tree item for traversing a [`TagTree`].
#[derive(Clone)]
pub struct TagTreeItem<'a> {
    item_type: TagTreeItemType,
    path: Cow<'a, str>,
    tag_group: Option<TagGroup>,
    tag_tree: &'a dyn TagTree
}

impl<'a> TagTreeItem<'a> {
    /// Instantiates a new item for the target tag tree.
    ///
    /// # Panics
    ///
    /// Panics if tag_group is set for a directory or not set for a tag.
    pub fn new(item_type: TagTreeItemType, path: Cow<'a, str>, tag_group: Option<TagGroup>, tag_tree: &'a dyn TagTree) -> Self {
        assert!((item_type == TagTreeItemType::Tag) ^ tag_group.is_none());
        Self {
            item_type, path, tag_group, tag_tree
        }
    }

    /// Get the type of item this is.
    pub fn item_type(&self) -> TagTreeItemType {
        self.item_type
    }

    /// Return `true` if this is a tag.
    pub fn is_tag(&self) -> bool {
        self.item_type == TagTreeItemType::Tag
    }

    /// Return `true` if this is a directory.
    pub fn is_directory(&self) -> bool {
        self.item_type == TagTreeItemType::Directory
    }

    /// Get the inner files of this directory.
    ///
    /// Returns `None` if this is not a directory or it no longer exists.
    pub fn files(&self) -> Option<Vec<TagTreeItem<'a>>> {
        if self.item_type == TagTreeItemType::Directory {
            self.tag_tree.files_in_path(&self.path)
        }
        else {
            None
        }
    }

    /// Get the tag group if this is a tag.
    ///
    /// Returns `None` if this is not a tag.
    pub fn tag_group(&self) -> Option<TagGroup> {
        self.tag_group
    }

    /// Get the path as a string, excluding extensions if it is a tag path.
    pub fn path_str(&self) -> &str {
        &self.path
    }

    /// Get this as a tag path, if it is a tag.
    ///
    /// Returns `None` if this is not a tag.
    pub fn tag_path(&self) -> Option<TagPath> {
        if self.is_directory() {
            return None
        }
        Some(TagPath::new(&self.path, self.tag_group.unwrap()).unwrap())
    }
}

#[derive(PartialEq)]
pub enum CachingTagTreeWriteStrategy {
    /// Writing a tag instantly commits it into the file system.
    Instant,

    /// Writing a tag only impacts what is cached and does not actually write to the delegate.
    Manual
}

pub struct CachingTagTree<T> where T: TagTree {
    inner: T,

    // wrapped in Mutex to allow writing to state even in immutable references
    tag_cache: Mutex<HashMap<TagPath, Arc<Mutex<Box<dyn PrimaryTagStructDyn>>>>>,
    strategy: CachingTagTreeWriteStrategy
}

impl<T: TagTree> CachingTagTree<T> {
    /// Wrap a tag tree with a cache.
    pub fn new(inner: T, strategy: CachingTagTreeWriteStrategy) -> Self {
        Self {
            inner,
            tag_cache: Mutex::new(HashMap::new()),
            strategy
        }
    }

    /// Get the inner instance as a reference.
    pub fn inner(&self) -> &T {
        &self.inner
    }

    /// Get the inner instance as a mutable reference.
    pub fn inner_mut(&mut self) -> &mut T {
        &mut self.inner
    }

    /// Consume the cache and return the inner value.
    pub fn into_inner(self) -> T {
        self.inner
    }

    /// Get a direct reference to the tag in the cache.
    ///
    /// Returns `None` if no such tag is cached.
    pub fn get(&mut self, path: &TagPath) -> Option<Arc<Mutex<Box<dyn PrimaryTagStructDyn>>>> {
        self.tag_cache
            .lock()
            .unwrap()
            .get(path)
            .map(Clone::clone)
    }

    /// Evict a tag from the tag cache and return it if it existed.
    ///
    /// Returns `None` if no such tag was found in the cache.
    pub fn evict(&mut self, path: &TagPath) -> Option<Box<dyn PrimaryTagStructDyn>> {
        self.tag_cache
            .lock()
            .unwrap()
            .remove(path)
            .map(|tag| Arc::into_inner(tag).unwrap().into_inner().unwrap())
    }

    /// Write the tag to the delegate.
    ///
    /// Returns `Err(Error::TagNotFound)` if the tag is not open, or some other [`Error`] if an error occurs on the delegate.
    pub fn commit(&mut self, path: &TagPath) -> RinghopperResult<()> {
        let cache = self.tag_cache.lock().unwrap();
        let tag = cache.get(path).ok_or_else(|| Error::TagNotFound(path.clone()))?;
        self.inner.write_tag(path, tag.as_ref().lock().unwrap().as_ref())?;
        Ok(())
    }

    /// Write all tags to the delegate.
    ///
    /// Returns a vector of all tags that couldn't be written, with a corresponding [`Error`].
    pub fn commit_all(&mut self) -> Vec<(TagPath, Error)> {
        let cache = self.tag_cache.lock().unwrap();
        let mut errors = Vec::new();

        for (k, v) in cache.iter() {
            match self.inner.write_tag(k, v.lock().unwrap().clone_inner().as_ref()) {
                Ok(_) => (),
                Err(e) => errors.push((k.to_owned(), e))
            }
        }

        errors
    }
}

impl<T: TagTree> TagTree for CachingTagTree<T> {
    fn open_tag_copy(&self, path: &TagPath) -> RinghopperResult<Box<dyn PrimaryTagStructDyn>> {
        self.open_tag_shared(path)
            .map(|tag| tag.lock().unwrap().clone_inner())
    }
    fn open_tag_shared(&self, path: &TagPath) -> RinghopperResult<Arc<Mutex<Box<dyn PrimaryTagStructDyn>>>> {
        let mut cache = self.tag_cache.lock().unwrap();
        if let Some(n) = cache.get(path) {
            return Ok(n.clone())
        }
        let result = self.inner.open_tag_copy(path)?;
        let cached = Arc::new(Mutex::new(result));
        cache.insert(path.clone(), cached.clone());
        Ok(cached)
    }
    fn files_in_path(&self, path: &str) -> Option<Vec<TagTreeItem>> {
        self.inner.files_in_path(path)
    }
    fn write_tag(&mut self, path: &TagPath, tag: &dyn PrimaryTagStructDyn) -> RinghopperResult<()> {
        if self.strategy == CachingTagTreeWriteStrategy::Instant {
            self.inner.write_tag(path, tag)?;
        }
        self.tag_cache.lock().unwrap().insert(path.to_owned(), Arc::new(Mutex::new(tag.clone_inner())));
        Ok(())
    }
}

pub struct VirtualTagDirectory {
    directories: Vec<PathBuf>
}

impl VirtualTagDirectory {
    /// Initialize a virtual tags directory.
    ///
    /// Lower directories have higher priority and are chosen first, and it is where tags will be
    /// written to by default.
    ///
    /// Returns `Error::InvalidTagsDirectory` if any directories passed do not exist.
    pub fn new<P: AsRef<Path>>(directories: &[P]) -> RinghopperResult<Self> {
        let directories: Vec<PathBuf> = directories.iter().map(|path| path.as_ref().to_path_buf()).collect();

        for i in &directories {
            if !i.is_dir() {
                return Err(Error::InvalidTagsDirectory)
            }
        }

        Ok(Self { directories })
    }

    fn path_for_tag(&self, path: &TagPath) -> Option<PathBuf> {
        let native_path = path.to_native_path();
        for index in 0..self.directories.len() {
            let directory = &self.directories[index];
            let path_to_test = directory.join(&native_path);
            if !path_to_test.exists() {
                continue
            }
            return Some(path_to_test);
        }
        None
    }
}

impl TagTree for VirtualTagDirectory {
    fn open_tag_copy(&self, path: &TagPath) -> RinghopperResult<Box<dyn PrimaryTagStructDyn>> {
        let file_path = self.path_for_tag(path).ok_or_else(|| Error::TagNotFound(path.clone()))?;
        let file = read(file_path).map_err(|_| Error::FailedToReadFile)?;
        return ringhopper_structs::read_any_tag_from_file_buffer(&file, ParseStrictness::Strict)
            .map_err(|e| match e {
                Error::TagParseFailure => Error::CorruptedTag(path.clone()),
                e => e
            })
    }
    fn files_in_path(&self, path: &str) -> Option<Vec<TagTreeItem>> {
        let mut result = Vec::new();
        let mut success = false;

        for dir_path in &self.directories {
            let dir = match std::fs::read_dir(dir_path.join(path)) {
                Ok(n) => n,
                Err(_) => continue
            };
            success = true;

            let entries = dir
                .filter_map(|f| f.ok())
                .filter_map(|f| {
                    let path = f.path();
                    let is_dir = path.is_dir();
                    if !is_dir && !path.is_file() {
                        return None
                    }

                    let mut path = path.strip_prefix(dir_path).unwrap().to_owned().into_os_string().into_string().ok()?;
                    if !is_dir {
                        let extension = path.rfind('.')?;
                        let tag_group = TagGroup::from_str(&path[extension + 1..]).ok()?;
                        path.truncate(extension);
                        Some(TagTreeItem::new(
                            TagTreeItemType::Tag,
                            Cow::Owned(path),
                            Some(tag_group),
                            self
                        ))
                    }

                    else {
                        Some(TagTreeItem::new(
                            TagTreeItemType::Directory,
                            Cow::Owned(path),
                            None,
                            self
                        ))
                    }
                });

            for f in entries {
                result.push(f);
            }
        }

        result.dedup_by(|a, b| {
            a.item_type == b.item_type && a.tag_group == b.tag_group && a.path == b.path
        });

        if !success {
            return None
        }

        Some(result)
    }
    fn write_tag(&mut self, path: &TagPath, tag: &dyn PrimaryTagStructDyn) -> RinghopperResult<()> {
        let file_to_write_to = self.path_for_tag(path).unwrap_or_else(|| self.directories[0].join(path.to_native_path()));
        std::fs::create_dir_all(file_to_write_to.parent().unwrap()).map_err(|_| Error::FailedToWriteFile)?;
        write(file_to_write_to, tag.to_tag_file()?).map_err(|_| Error::FailedToReadFile)
    }
}

#[cfg(test)]
mod test;

#[cfg(test)]
pub(crate) use self::test::MockTagTree;