#!/usr/bin/env python3
"""
Standalone PPTX merger for Slideflow.
Merges specific slides from multiple presentations using XML manipulation.
"""

import sys
import json
import zipfile
import shutil
import tempfile
from pathlib import Path
from collections import defaultdict


def merge_slides(output_path, slide_specs):
    """
    Merge slides from multiple presentations by manipulating PPTX XML.

    Args:
        output_path: Output PPTX file path
        slide_specs: List of {"path": "file.pptx", "slides": [1,2,3]}

    Returns:
        JSON result with success/error
    """
    try:
        # Create temp directory for working files
        temp_dir = Path(tempfile.mkdtemp())
        merged_dir = temp_dir / "merged"
        merged_dir.mkdir()

        # Use first presentation as base template
        first_spec = slide_specs[0]
        base_path = first_spec["path"]

        if not Path(base_path).exists():
            return json.dumps({"success": False, "error": f"Base file not found: {base_path}"})

        # Extract base presentation structure
        with zipfile.ZipFile(base_path, 'r') as zip_ref:
            zip_ref.extractall(merged_dir)

        # Remove all existing slides from base
        slides_dir = merged_dir / "ppt" / "slides"
        slides_rels_dir = merged_dir / "ppt" / "slides" / "_rels"

        if slides_dir.exists():
            for slide_file in slides_dir.glob("slide*.xml"):
                slide_file.unlink()

        if slides_rels_dir.exists():
            for rels_file in slides_rels_dir.glob("slide*.xml.rels"):
                rels_file.unlink()

        slides_dir.mkdir(parents=True, exist_ok=True)
        slides_rels_dir.mkdir(parents=True, exist_ok=True)

        # Copy selected slides from each source
        slide_counter = 1
        slides_added = 0
        media_counter = 1
        media_map = {}  # Track media files to avoid duplicates

        for spec in slide_specs:
            source_path = spec["path"]
            slide_numbers = spec["slides"]

            if not Path(source_path).exists():
                continue

            # Extract source presentation
            source_dir = temp_dir / f"source_{slides_added}"
            with zipfile.ZipFile(source_path, 'r') as zip_ref:
                zip_ref.extractall(source_dir)

            for slide_num in slide_numbers:
                # Copy slide XML
                source_slide = source_dir / "ppt" / "slides" / f"slide{slide_num}.xml"
                if not source_slide.exists():
                    continue

                dest_slide = slides_dir / f"slide{slide_counter}.xml"
                shutil.copy2(source_slide, dest_slide)

                # Copy slide relationships if they exist
                source_rels = source_dir / "ppt" / "slides" / "_rels" / f"slide{slide_num}.xml.rels"
                if source_rels.exists():
                    dest_rels = slides_rels_dir / f"slide{slide_counter}.xml.rels"
                    shutil.copy2(source_rels, dest_rels)

                # Copy media files referenced by this slide
                source_media_dir = source_dir / "ppt" / "media"
                if source_media_dir.exists():
                    dest_media_dir = merged_dir / "ppt" / "media"
                    dest_media_dir.mkdir(parents=True, exist_ok=True)

                    for media_file in source_media_dir.iterdir():
                        if media_file.is_file():
                            dest_media = dest_media_dir / media_file.name
                            if not dest_media.exists():
                                shutil.copy2(media_file, dest_media)

                slide_counter += 1
                slides_added += 1

        # Update presentation.xml with correct slide count and relationships
        update_presentation_xml(merged_dir, slides_added)

        # Update presentation.xml.rels with slide relationships
        update_presentation_rels(merged_dir, slides_added)

        # Update [Content_Types].xml
        update_content_types(merged_dir, slides_added)

        # Zip everything back into PPTX
        with zipfile.ZipFile(output_path, 'w', zipfile.ZIP_DEFLATED) as zipf:
            for file_path in merged_dir.rglob('*'):
                if file_path.is_file():
                    arcname = file_path.relative_to(merged_dir)
                    zipf.write(file_path, arcname)

        # Cleanup
        shutil.rmtree(temp_dir)

        return json.dumps({
            "success": True,
            "slides_added": slides_added,
            "output": output_path
        })

    except Exception as e:
        import traceback
        return json.dumps({
            "success": False,
            "error": str(e),
            "traceback": traceback.format_exc()
        })


def update_presentation_xml(merged_dir, slide_count):
    """Update presentation.xml with correct slide references."""
    import xml.etree.ElementTree as ET

    pres_file = merged_dir / "ppt" / "presentation.xml"
    tree = ET.parse(pres_file)
    root = tree.getroot()

    # Find sldIdLst element
    ns = {'p': 'http://schemas.openxmlformats.org/presentationml/2006/main',
          'r': 'http://schemas.openxmlformats.org/officeDocument/2006/relationships'}

    sld_id_lst = root.find('.//p:sldIdLst', ns)
    if sld_id_lst is not None:
        # Clear existing slides
        sld_id_lst.clear()

        # Add new slide references
        for i in range(1, slide_count + 1):
            sld_id = ET.SubElement(sld_id_lst, f'{{{ns["p"]}}}sldId')
            sld_id.set('id', str(255 + i))
            sld_id.set(f'{{{ns["r"]}}}id', f'rId{i}')

    tree.write(pres_file, encoding='UTF-8', xml_declaration=True)


def update_presentation_rels(merged_dir, slide_count):
    """Update presentation.xml.rels with slide relationships."""
    import xml.etree.ElementTree as ET

    rels_file = merged_dir / "ppt" / "_rels" / "presentation.xml.rels"
    tree = ET.parse(rels_file)
    root = tree.getroot()

    ns = {'': 'http://schemas.openxmlformats.org/package/2006/relationships'}
    ET.register_namespace('', ns[''])

    # Remove existing slide relationships
    for rel in list(root.findall('.//Relationship', ns)):
        rel_type = rel.get('Type', '')
        if '/slide' in rel_type and 'slideMaster' not in rel_type:
            root.remove(rel)

    # Add new slide relationships
    for i in range(1, slide_count + 1):
        rel = ET.SubElement(root, 'Relationship')
        rel.set('Id', f'rId{i}')
        rel.set('Type', 'http://schemas.openxmlformats.org/officeDocument/2006/relationships/slide')
        rel.set('Target', f'slides/slide{i}.xml')

    tree.write(rels_file, encoding='UTF-8', xml_declaration=True)


def update_content_types(merged_dir, slide_count):
    """Update [Content_Types].xml with slide references."""
    import xml.etree.ElementTree as ET

    content_types_file = merged_dir / "[Content_Types].xml"
    tree = ET.parse(content_types_file)
    root = tree.getroot()

    ns = {'': 'http://schemas.openxmlformats.org/package/2006/content-types'}
    ET.register_namespace('', ns[''])

    # Remove existing slide Override elements
    for override in list(root.findall('.//Override', ns)):
        part_name = override.get('PartName', '')
        if '/slides/slide' in part_name:
            root.remove(override)

    # Add new slide Override elements
    for i in range(1, slide_count + 1):
        override = ET.SubElement(root, 'Override')
        override.set('PartName', f'/ppt/slides/slide{i}.xml')
        override.set('ContentType', 'application/vnd.openxmlformats-officedocument.presentationml.slide+xml')

    tree.write(content_types_file, encoding='UTF-8', xml_declaration=True)


if __name__ == "__main__":
    if len(sys.argv) < 3:
        print(json.dumps({"error": "Usage: merge_pptx.py output.pptx '{\"specs\": [...]}}'"}))
        sys.exit(1)

    output_path = sys.argv[1]
    specs_json = sys.argv[2]

    try:
        data = json.loads(specs_json)
        slide_specs = data["specs"]
    except (json.JSONDecodeError, KeyError) as e:
        print(json.dumps({"error": f"Invalid JSON: {e}"}))
        sys.exit(1)

    result = merge_slides(output_path, slide_specs)
    print(result)